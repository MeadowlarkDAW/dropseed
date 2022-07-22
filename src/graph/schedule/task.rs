use smallvec::SmallVec;
use std::fmt::{Debug, Error, Formatter, Write};

use crate::graph::buffers::events::{NoteEvent, ParamEvent};
use crate::graph::buffers::plugin::PluginEventIoBuffers;
use dropseed_core::plugin::buffer::SharedBuffer;
use dropseed_core::plugin::{ProcBuffers, ProcInfo};

use crate::graph::shared_pool::{SharedDelayCompNode, SharedPluginHostAudioThread};

use super::sum::SumTask;

pub(crate) enum Task {
    Plugin(PluginTask),
    DelayComp(DelayCompTask),
    Sum(SumTask),
    DeactivatedPlugin(DeactivatedPluginTask),
}

impl Debug for Task {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        match self {
            Task::Plugin(t) => {
                let mut f = f.debug_struct("Plugin");

                f.field("id", &t.plugin.id());

                if !t.buffers.audio_in.is_empty() {
                    let mut s = String::new();
                    for b in t.buffers.audio_in.iter() {
                        write!(s, "{:?}, ", b)?;
                    }

                    f.field("audio_in", &s);
                }

                if !t.buffers.audio_out.is_empty() {
                    let mut s = String::new();
                    for b in t.buffers.audio_out.iter() {
                        write!(s, "{:?}, ", b)?;
                    }

                    f.field("audio_out", &s);
                }

                if let Some(automation_in_buffers) = &t.event_buffers.unmixed_param_in_buffers {
                    let mut s = String::new();
                    for b in automation_in_buffers.iter() {
                        s.push_str(&format!("{:?}, ", b.id()))
                    }

                    f.field("automation_in", &s);
                }

                if let Some(automation_out_buffer) = &t.event_buffers.param_out_buffer {
                    f.field("automation_out", &format!("{:?}", automation_out_buffer.id()));
                }

                if !t.event_buffers.unmixed_note_in_buffers.is_empty() {
                    let mut has_buffer = false;
                    let mut s = String::new();
                    for buffers in t.event_buffers.unmixed_note_in_buffers.iter().flatten() {
                        has_buffer = true;

                        s.push('[');

                        for b in buffers.iter() {
                            s.push_str(&format!("{:?}, ", b.id()))
                        }

                        s.push_str("], ");
                    }

                    if has_buffer {
                        f.field("note_in", &s);
                    }
                }

                if !t.event_buffers.note_out_buffers.is_empty() {
                    let mut has_buffer = false;
                    let mut s = String::new();
                    for buffer in t.event_buffers.note_out_buffers.iter().flatten() {
                        has_buffer = true;

                        s.push_str(&format!("{:?}, ", buffer.id()));
                    }

                    if has_buffer {
                        f.field("note_out", &s);
                    }
                }

                f.finish()
            }
            Task::DelayComp(t) => {
                let mut f = f.debug_struct("DelayComp");

                f.field("audio_in", &t.audio_in.id());
                f.field("audio_out", &t.audio_out.id());
                f.field("delay", &t.delay_comp_node.delay());

                f.finish()
            }
            Task::Sum(t) => {
                let mut f = f.debug_struct("Sum");

                let mut s = String::new();
                for b in t.audio_in.iter() {
                    write!(s, "{:?}, ", b.id())?;
                }
                f.field("audio_in", &s);

                f.field("audio_out", &format!("{:?}", t.audio_out.id()));

                f.finish()
            }
            Task::DeactivatedPlugin(t) => {
                let mut f = f.debug_struct("DeactivatedPlugin");

                let mut s = String::new();
                for (b_in, b_out) in t.audio_through.iter() {
                    s.push_str(&format!("(in: {:?}, out: {:?})", b_in.id(), b_out.id()));
                }
                f.field("audio_through", &s);

                let mut s = String::new();
                for b in t.extra_audio_out.iter() {
                    s.push_str(&format!("{:?}, ", b.id()))
                }
                f.field("extra_audio_out", &s);

                if let Some(automation_out_buffer) = &t.automation_out_buffer {
                    f.field("automation_out", &format!("{:?}", automation_out_buffer.id()));
                }

                if !t.note_out_buffers.is_empty() {
                    let mut has_buffer = false;
                    let mut s = String::new();
                    for buffer in t.note_out_buffers.iter().flatten() {
                        has_buffer = true;

                        s.push_str(&format!("{:?}, ", buffer.id()));
                    }

                    if has_buffer {
                        f.field("note_out", &s);
                    }
                }

                f.finish()
            }
        }
    }
}

impl Task {
    pub fn process(&mut self, proc_info: &ProcInfo) {
        match self {
            Task::Plugin(task) => task.process(proc_info),
            Task::DelayComp(task) => task.process(proc_info),
            Task::Sum(task) => task.process(proc_info),
            Task::DeactivatedPlugin(task) => task.process(proc_info),
        }
    }
}

pub(crate) struct PluginTask {
    pub plugin: SharedPluginHostAudioThread,

    pub buffers: ProcBuffers,

    pub event_buffers: PluginEventIoBuffers,
}

impl PluginTask {
    fn process(&mut self, proc_info: &ProcInfo) {
        let mut plugin_audio_thread = self.plugin.plugin.borrow_mut();

        plugin_audio_thread.process(proc_info, &mut self.buffers, &mut self.event_buffers);
    }
}

pub(crate) struct DelayCompTask {
    pub delay_comp_node: SharedDelayCompNode,

    pub audio_in: SharedBuffer<f32>,
    pub audio_out: SharedBuffer<f32>,
}

impl DelayCompTask {
    fn process(&mut self, proc_info: &ProcInfo) {
        let mut delay_comp_node = self.delay_comp_node.node.borrow_mut();

        delay_comp_node.process(proc_info, &self.audio_in, &self.audio_out);
    }
}

pub(crate) struct DeactivatedPluginTask {
    pub audio_through: SmallVec<[(SharedBuffer<f32>, SharedBuffer<f32>); 4]>,
    pub extra_audio_out: SmallVec<[SharedBuffer<f32>; 4]>,

    pub automation_out_buffer: Option<SharedBuffer<ParamEvent>>,

    pub note_out_buffers: SmallVec<[Option<SharedBuffer<NoteEvent>>; 2]>,
}

impl DeactivatedPluginTask {
    fn process(&mut self, proc_info: &ProcInfo) {
        // Pass audio through the main ports.
        for (in_buf, out_buf) in self.audio_through.iter() {
            out_buf.set_constant(in_buf.is_constant());

            let in_buf_ref = in_buf.borrow();
            let mut out_buf_ref = out_buf.borrow_mut();

            let in_buf = &in_buf_ref[0..proc_info.frames];
            let out_buf = &mut out_buf_ref[0..proc_info.frames];

            out_buf.copy_from_slice(in_buf);
        }

        // Make sure all output buffers are cleared.
        for out_buf in self.extra_audio_out.iter() {
            out_buf.clear_until(proc_info.frames);
        }
        if let Some(out_buf) = &self.automation_out_buffer {
            out_buf.truncate();
        }
        for out_buf in self.note_out_buffers.iter().flatten() {
            out_buf.truncate();
        }
    }
}
