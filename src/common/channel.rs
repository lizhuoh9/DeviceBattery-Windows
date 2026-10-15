//! The transport abstraction the protocol readers talk through.

use std::time::Duration;

use super::model::DeviceReading;

/// Why a report exchange failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelError {
    /// No matching input report arrived in time.
    Timeout,
    /// The OS returned an error code.
    Io(u32),
    /// The device went away or the report cannot be routed.
    Closed,
}

/// One HID interface (or a group of collections of it) that accepts output reports and
/// answers with input reports. Byte 0 of every request and reply is the report ID.
pub trait ReportChannel: Send {
    /// Sends `request` and returns the first input report `accept` claims as its answer.
    fn exchange(
        &mut self,
        request: &[u8],
        timeout: Duration,
        accept: &dyn Fn(&[u8]) -> bool,
    ) -> Result<Vec<u8>, ChannelError>;
}

/// Something that can report the batteries of the devices behind it.
pub trait BatteryReader: Send {
    /// Reads every device; `now` is the current time in seconds since the Unix epoch.
    fn read(&mut self, now: u64) -> Vec<DeviceReading>;
}

/// `bytes` zero-padded (or truncated) to `length`.
pub fn padded(bytes: &[u8], length: usize) -> Vec<u8> {
    let mut result = bytes.to_vec();
    result.resize(length, 0);
    result
}

/// The byte at `index`, or 0 when the report is shorter.
pub fn byte_at(report: &[u8], index: usize) -> u8 {
    report.get(index).copied().unwrap_or(0)
}

/// Uppercase hex without separators, e.g. `1A2B3C4D`.
pub fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

#[cfg(test)]
pub mod testing {
    //! A scripted channel for reader tests.

    use super::*;

    /// Answers each request through `respond`; records what was sent.
    pub struct FakeChannel<F: FnMut(&[u8]) -> Result<Vec<u8>, ChannelError> + Send> {
        respond: F,
        pub sent: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    }

    impl<F: FnMut(&[u8]) -> Result<Vec<u8>, ChannelError> + Send> FakeChannel<F> {
        pub fn new(respond: F) -> Self {
            Self {
                respond,
                sent: Default::default(),
            }
        }
    }

    impl<F: FnMut(&[u8]) -> Result<Vec<u8>, ChannelError> + Send> ReportChannel for FakeChannel<F> {
        fn exchange(
            &mut self,
            request: &[u8],
            _timeout: Duration,
            accept: &dyn Fn(&[u8]) -> bool,
        ) -> Result<Vec<u8>, ChannelError> {
            self.sent.lock().expect("sent log").push(request.to_vec());
            let reply = (self.respond)(request)?;
            // A reply the reader would not claim is the same as no reply at all.
            if accept(&reply) {
                Ok(reply)
            } else {
                Err(ChannelError::Timeout)
            }
        }
    }

    /// `bytes` zero-padded to `length`.
    pub fn report(bytes: &[u8], length: usize) -> Vec<u8> {
        padded(bytes, length)
    }
}
