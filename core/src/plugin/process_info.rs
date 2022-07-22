use smallvec::SmallVec;

use crate::transport::TransportInfo;

use super::buffer::{AudioPortBuffer, AudioPortBufferMut};

/// The status of a call to a plugin's `process()` method.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum ProcessStatus {
    /// Processing failed. The output buffer must be discarded.
    Error = 0,

    /// Processing succeeded, keep processing.
    Continue = 1,

    /// Processing succeeded, keep processing if the output is not quiet.
    ContinueIfNotQuiet = 2,

    /// Rely upon the plugin's tail to determine if the plugin should continue to process.
    /// see clap_plugin_tail
    Tail = 3,

    /// Processing succeeded, but no more processing is required until
    /// the next event or variation in audio input.
    Sleep = 4,
}

pub struct ProcInfo {
    /// A steady sample time counter.
    ///
    /// This field can be used to calculate the sleep duration between two process calls.
    /// This value may be specific to this plugin instance and have no relation to what
    /// other plugin instances may receive.
    ///
    /// This will be `-1` if not available, otherwise the value will be increased by
    /// at least `frames_count` for the next call to process.
    pub steady_time: i64,

    /// The number of frames to process. All buffers in this struct are gauranteed to be
    /// at-least this length.
    pub frames: usize,

    pub transport: TransportInfo,
}

pub struct ProcBuffers {
    pub audio_in: SmallVec<[AudioPortBuffer; 2]>,
    pub audio_out: SmallVec<[AudioPortBufferMut; 2]>,
}

impl ProcBuffers {
    pub fn audio_inputs_silent(&self, frames: usize) -> bool {
        for buf in self.audio_in.iter() {
            if !buf.is_silent(frames) {
                return false;
            }
        }
        true
    }

    pub fn clear_all_outputs(&mut self, proc_info: &ProcInfo) {
        for buf in self.audio_out.iter_mut() {
            buf.clear_all(proc_info.frames);
        }
    }
}
