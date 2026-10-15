//! Raw HID access: SetupAPI enumeration and overlapped report I/O with timeouts.
//!
//! On Windows every top-level collection is its own device, so a channel can span several
//! collections: requests go to the collection that declares their report ID, and replies are
//! read from all of them.

use std::ffi::c_void;
use std::mem::size_of;
use std::time::{Duration, Instant};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FlushQueue, HidD_FreePreparsedData,
    HidD_GetAttributes, HidD_GetHidGuid, HidD_GetPreparsedData, HidD_GetProductString,
    HidD_GetSerialNumberString, HidP_GetCaps, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::PCWSTR;

use super::text::{from_wide, wide};
use crate::common::channel::{ChannelError, ReportChannel, padded};

const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// One HID top-level collection.
#[derive(Clone, Debug)]
pub struct Collection {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_length: usize,
    pub output_length: usize,
    pub product: String,
    pub serial: String,
}

/// Every HID collection currently present.
pub fn enumerate() -> Vec<Collection> {
    // SAFETY: the GUID and interface data outlive the calls that use them, and the
    // device-info set is destroyed before returning.
    unsafe {
        let guid = HidD_GetHidGuid();
        let Ok(set) = SetupDiGetClassDevsW(
            Some(&guid),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        ) else {
            return Vec::new();
        };
        let mut collections = Vec::new();
        for index in 0u32.. {
            let mut interface = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(set, None, &guid, index, &mut interface).is_err() {
                break;
            }
            if let Some(path) = interface_path(set, &interface)
                && let Some(collection) = describe(&path)
            {
                collections.push(collection);
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
        collections
    }
}

fn interface_path(set: HDEVINFO, interface: &SP_DEVICE_INTERFACE_DATA) -> Option<String> {
    let mut required = 0u32;
    // SAFETY: the first call only reports the size; the buffer handed to the second call is
    // at least that large and u32-aligned, as the detail structure requires.
    unsafe {
        let _ =
            SetupDiGetDeviceInterfaceDetailW(set, interface, None, 0, Some(&mut required), None);
        if (required as usize) < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() {
            return None;
        }
        let mut buffer = vec![0u32; (required as usize).div_ceil(4)];
        let detail = buffer
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        SetupDiGetDeviceInterfaceDetailW(set, interface, Some(detail), required, None, None)
            .ok()?;
        let path = std::ptr::addr_of!((*detail).DevicePath).cast::<u16>();
        let units = (required as usize - size_of::<u32>()) / size_of::<u16>();
        Some(from_wide(std::slice::from_raw_parts(path, units)))
    }
}

/// Opens the collection without access rights (allowed even for collections the system
/// holds exclusively, like keyboards) and reads its IDs, usages and strings.
fn describe(path: &str) -> Option<Collection> {
    let wide_path = wide(path);
    // SAFETY: the path is NUL-terminated; the handle is closed before returning.
    unsafe {
        let handle = CreateFileW(
            PCWSTR(wide_path.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()?;
        let collection = query(handle, path);
        let _ = CloseHandle(handle);
        collection
    }
}

fn query(handle: HANDLE, path: &str) -> Option<Collection> {
    // SAFETY: every out-parameter points at a live local; preparsed data is freed right after use.
    unsafe {
        let mut attributes = HIDD_ATTRIBUTES {
            Size: size_of::<HIDD_ATTRIBUTES>() as u32,
            ..Default::default()
        };
        if !HidD_GetAttributes(handle, &mut attributes) {
            return None;
        }
        let mut preparsed = PHIDP_PREPARSED_DATA::default();
        if !HidD_GetPreparsedData(handle, &mut preparsed) {
            return None;
        }
        let mut caps = HIDP_CAPS::default();
        let status = HidP_GetCaps(preparsed, &mut caps);
        let _ = HidD_FreePreparsedData(preparsed);
        if status != HIDP_STATUS_SUCCESS {
            return None;
        }
        Some(Collection {
            path: path.to_owned(),
            vendor_id: attributes.VendorID,
            product_id: attributes.ProductID,
            usage_page: caps.UsagePage,
            usage: caps.Usage,
            input_length: usize::from(caps.InputReportByteLength),
            output_length: usize::from(caps.OutputReportByteLength),
            product: hid_string(handle, HidD_GetProductString),
            serial: hid_string(handle, HidD_GetSerialNumberString),
        })
    }
}

fn hid_string(handle: HANDLE, getter: unsafe fn(HANDLE, *mut c_void, u32) -> bool) -> String {
    let mut buffer = [0u16; 128];
    // SAFETY: the buffer length is passed in bytes, as these HID string getters expect.
    let ok = unsafe {
        getter(
            handle,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * size_of::<u16>()) as u32,
        )
    };
    if ok {
        from_wide(&buffer).trim().to_owned()
    } else {
        String::new()
    }
}

/// An open collection with one outstanding overlapped read.
struct HidHandle {
    handle: HANDLE,
    read_event: HANDLE,
    write_event: HANDLE,
    /// Boxed so the OVERLAPPED and buffer stay put while the kernel writes into them.
    pending: Box<PendingRead>,
    reading: bool,
    output_length: usize,
}

struct PendingRead {
    overlapped: OVERLAPPED,
    buffer: Vec<u8>,
}

// SAFETY: the handles are kernel objects usable from any thread; a HidHandle is only ever
// used by one thread at a time (it lives inside a channel owned by the monitor thread).
unsafe impl Send for HidHandle {}

impl HidHandle {
    fn open(collection: &Collection) -> Result<Self, ChannelError> {
        let path = wide(&collection.path);
        // SAFETY: the path is NUL-terminated; partially created handles are closed on error.
        unsafe {
            let handle = CreateFileW(
                PCWSTR(path.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
            .map_err(io_error)?;
            let events = (
                CreateEventW(None, true, false, PCWSTR::null()),
                CreateEventW(None, true, false, PCWSTR::null()),
            );
            let (Ok(read_event), Ok(write_event)) = events else {
                if let Ok(event) = events.0 {
                    let _ = CloseHandle(event);
                }
                if let Ok(event) = events.1 {
                    let _ = CloseHandle(event);
                }
                let _ = CloseHandle(handle);
                return Err(ChannelError::Closed);
            };
            Ok(Self {
                handle,
                read_event,
                write_event,
                pending: Box::new(PendingRead {
                    overlapped: OVERLAPPED::default(),
                    buffer: vec![0; collection.input_length.max(1)],
                }),
                reading: false,
                output_length: collection.output_length,
            })
        }
    }

    /// Writes one output report, padded to the collection's report length.
    fn write(&mut self, report: &[u8]) -> Result<(), ChannelError> {
        let data = padded(report, self.output_length);
        let mut overlapped = OVERLAPPED {
            hEvent: self.write_event,
            ..Default::default()
        };
        // SAFETY: `data` and `overlapped` outlive the I/O: GetOverlappedResult waits for it to
        // finish (or for the cancellation to land) before they are dropped.
        unsafe {
            match WriteFile(self.handle, Some(&data), None, Some(&mut overlapped)) {
                Ok(()) => {}
                Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {
                    if WaitForSingleObject(self.write_event, millis(WRITE_TIMEOUT)) != WAIT_OBJECT_0
                    {
                        let _ = CancelIoEx(self.handle, Some(&overlapped));
                    }
                }
                Err(error) => return Err(io_error(error)),
            }
            let mut written = 0u32;
            GetOverlappedResult(self.handle, &overlapped, &mut written, true).map_err(io_error)
        }
    }

    /// Makes sure a read is outstanding; its completion signals `read_event`.
    fn start_read(&mut self) -> Result<(), ChannelError> {
        if self.reading {
            return Ok(());
        }
        let pending = &mut *self.pending;
        pending.overlapped = OVERLAPPED {
            hEvent: self.read_event,
            ..Default::default()
        };
        // SAFETY: the buffer and OVERLAPPED live in a Box that is not touched until the read
        // completes or is cancelled (see `finish_read` and `Drop`).
        let result = unsafe {
            ReadFile(
                self.handle,
                Some(&mut pending.buffer),
                None,
                Some(&mut pending.overlapped),
            )
        };
        match result {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(error) => return Err(io_error(error)),
        }
        self.reading = true;
        Ok(())
    }

    /// Collects the report of a read whose event fired.
    fn finish_read(&mut self) -> Result<Vec<u8>, ChannelError> {
        let mut transferred = 0u32;
        // SAFETY: the read has completed (its event is signaled), so the buffer is ours again.
        let result = unsafe {
            GetOverlappedResult(
                self.handle,
                &self.pending.overlapped,
                &mut transferred,
                false,
            )
        };
        self.reading = false;
        result.map_err(io_error)?;
        Ok(self.pending.buffer[..(transferred as usize).min(self.pending.buffer.len())].to_vec())
    }

    /// Drops input reports that arrived before the request.
    fn flush(&self) {
        // SAFETY: plain call on a valid handle.
        unsafe {
            let _ = HidD_FlushQueue(self.handle);
        }
    }
}

impl Drop for HidHandle {
    fn drop(&mut self) {
        // SAFETY: an outstanding read is cancelled and awaited before its buffer is freed.
        unsafe {
            if self.reading {
                let _ = CancelIoEx(self.handle, Some(&self.pending.overlapped));
                let mut transferred = 0u32;
                let _ = GetOverlappedResult(
                    self.handle,
                    &self.pending.overlapped,
                    &mut transferred,
                    true,
                );
            }
            let _ = CloseHandle(self.read_event);
            let _ = CloseHandle(self.write_event);
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Collections of one interface, used together.
pub struct HidChannel {
    handles: Vec<HidHandle>,
    /// Report ID → index of the handle whose collection declares it.
    routes: Vec<(u8, usize)>,
}

impl HidChannel {
    /// Opens each collection; each is paired with the report ID it carries.
    pub fn open(collections: &[(u8, Collection)]) -> Result<Self, ChannelError> {
        let mut handles = Vec::with_capacity(collections.len());
        let mut routes = Vec::with_capacity(collections.len());
        for (report_id, collection) in collections {
            routes.push((*report_id, handles.len()));
            handles.push(HidHandle::open(collection)?);
        }
        Ok(Self { handles, routes })
    }
}

impl ReportChannel for HidChannel {
    fn exchange(
        &mut self,
        request: &[u8],
        timeout: Duration,
        accept: &dyn Fn(&[u8]) -> bool,
    ) -> Result<Vec<u8>, ChannelError> {
        let report_id = *request.first().ok_or(ChannelError::Closed)?;
        let index = self
            .routes
            .iter()
            .find(|(id, _)| *id == report_id)
            .map(|(_, index)| *index)
            .ok_or(ChannelError::Closed)?;
        self.handles.iter().for_each(HidHandle::flush);
        self.handles[index].write(request)?;

        let deadline = Instant::now() + timeout;
        loop {
            for handle in &mut self.handles {
                handle.start_read()?;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ChannelError::Timeout);
            }
            let events: Vec<HANDLE> = self
                .handles
                .iter()
                .map(|handle| handle.read_event)
                .collect();
            // SAFETY: all event handles stay open for the duration of the wait.
            let signaled = unsafe { WaitForMultipleObjects(&events, false, millis(remaining)) };
            if signaled == WAIT_TIMEOUT {
                return Err(ChannelError::Timeout);
            }
            let which = signaled.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            let handle = self
                .handles
                .get_mut(which)
                .ok_or(ChannelError::Io(signaled.0))?;
            let report = handle.finish_read()?;
            if accept(&report) {
                return Ok(report);
            }
        }
    }
}

fn millis(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

fn io_error(error: windows::core::Error) -> ChannelError {
    ChannelError::Io(error.code().0 as u32)
}
