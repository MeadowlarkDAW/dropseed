use atomic_refcell::AtomicRefCell;
use basedrop::{Shared, SharedCell};
use smallvec::SmallVec;

use dropseed_core::plugin::buffer::SharedBuffer;
use dropseed_core::plugin::ProcInfo;

use crate::utils::thread_id::SharedThreadIDs;

pub(crate) mod delay_comp_node;
pub(crate) mod sum;
pub(crate) mod task;
pub(crate) mod transport_task;

use task::Task;
use transport_task::TransportTask;

pub struct Schedule {
    pub(crate) tasks: Vec<Task>,

    pub(crate) graph_audio_in: SmallVec<[SharedBuffer<f32>; 4]>,
    pub(crate) graph_audio_out: SmallVec<[SharedBuffer<f32>; 4]>,

    pub(crate) max_block_size: usize,

    pub(crate) shared_transport_task: Shared<AtomicRefCell<TransportTask>>,
}

impl Schedule {
    pub(crate) fn new(
        max_block_size: usize,
        shared_transport_task: Shared<AtomicRefCell<TransportTask>>,
    ) -> Self {
        Self {
            tasks: Vec::new(),
            graph_audio_in: SmallVec::new(),
            graph_audio_out: SmallVec::new(),
            max_block_size,
            shared_transport_task,
        }
    }
}

impl std::fmt::Debug for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();

        s.push_str("Schedule {\n");

        let mut g_s = String::new();
        for b in self.graph_audio_in.iter() {
            g_s.push_str(&format!("{:?}, ", b.info()))
        }
        s.push_str(format!("    graph_audio_in: {:?},\n", &g_s).as_str());

        for t in self.tasks.iter() {
            s.push_str(format!("    {:?},\n", t).as_str());
        }

        let mut g_s = String::new();
        for b in self.graph_audio_out.iter() {
            g_s.push_str(&format!("{:?}, ", b.info()))
        }
        s.push_str(format!("    graph_audio_out: {:?},\n}}", &g_s).as_str());

        write!(f, "{}", s)
    }
}

impl Schedule {
    pub fn process_interleaved(
        &mut self,
        audio_in: &[f32],
        audio_in_channels: usize,
        audio_out: &mut [f32],
        audio_out_channels: usize,
    ) {
        if audio_in_channels != 0 && audio_out_channels != 0 {
            assert_eq!(audio_in.len() / audio_in_channels, audio_out.len() / audio_out_channels);
        }

        let total_frames = if audio_in_channels > 0 {
            let total_frames = audio_in.len() / audio_in_channels;

            assert_eq!(audio_out.len(), audio_out_channels * total_frames);

            total_frames
        } else if audio_out_channels > 0 {
            audio_out.len() / audio_out_channels
        } else {
            return;
        };

        if total_frames == 0 {
            return;
        }

        let mut processed_frames = 0;
        while processed_frames < total_frames {
            let frames = (total_frames - processed_frames).min(self.max_block_size);

            let transport = {
                let mut transport_task = AtomicRefCell::borrow_mut(&*self.shared_transport_task);
                transport_task.process(frames)
            };

            let proc_info = ProcInfo {
                steady_time: -1, // TODO
                frames,
                transport,
            };

            for (ch_i, in_buffer) in self.graph_audio_in.iter().enumerate() {
                if ch_i < audio_in_channels {
                    let mut buffer_ref = in_buffer.borrow_mut();

                    let buffer = &mut buffer_ref[0..frames];

                    for i in 0..proc_info.frames {
                        buffer[i] = audio_in[(i * audio_in_channels) + ch_i];
                    }

                    let mut is_constant = true;
                    let first_val = buffer[0];
                    for frame in &buffer[0..frames] {
                        if *frame != first_val {
                            is_constant = false;
                            break;
                        }
                    }

                    in_buffer.set_constant(is_constant);
                } else {
                    in_buffer.clear_until(frames);
                }
            }

            for task in self.tasks.iter_mut() {
                task.process(&proc_info)
            }

            let out_part = &mut audio_out[(processed_frames * audio_out_channels)
                ..((processed_frames + frames) * audio_out_channels)];
            for ch_i in 0..audio_out_channels {
                if let Some(buffer) = self.graph_audio_out.get(ch_i) {
                    let mut buffer_ref = buffer.borrow_mut();

                    let buffer = &mut buffer_ref[0..frames];

                    for i in 0..frames {
                        out_part[(i * audio_out_channels) + ch_i] = buffer[i];
                    }
                } else {
                    for i in 0..frames {
                        out_part[(i * audio_out_channels) + ch_i] = 0.0;
                    }
                }
            }

            processed_frames += frames;
        }
    }
}

// Required so we can send the schedule from the main thread to the process
// thread.
//
// This is safe because the schedule is only ever dereferenced in the process
// thread. The only reason why the main thread holds onto these shared
// pointers of buffers and `PluginAudioThread`s is so it can construct new
// schedules with them. The main thread never dereferences these pointers.
unsafe impl Send for Schedule {}
// Required so we can send the schedule from the main thread to the process
// thread. The fact that the main thread holds onto shared pointers of
// buffers and `PluginAudioThread`s requires this to be `Sync` as well.
//
// This is safe because the schedule is only ever dereferenced in the process
// thread. The only reason why the main thread holds onto these shared
// pointers of buffers and `PluginAudioThread`s is so it can construct new
// schedules with them. The main thread never dereferences these pointers.
unsafe impl Sync for Schedule {}

pub(crate) struct SharedSchedule {
    schedule: Shared<SharedCell<AtomicRefCell<Schedule>>>,
    thread_ids: SharedThreadIDs,
    coll_handle: basedrop::Handle,
}

// Implement Debug so we can send it in an event.
impl std::fmt::Debug for SharedSchedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedSchedule")
    }
}

impl SharedSchedule {
    pub(crate) fn new(
        schedule: Schedule,
        thread_ids: SharedThreadIDs,
        coll_handle: &basedrop::Handle,
    ) -> (Self, Self) {
        let schedule = Shared::new(
            coll_handle,
            SharedCell::new(Shared::new(coll_handle, AtomicRefCell::new(schedule))),
        );

        (
            Self {
                schedule: schedule.clone(),
                thread_ids: thread_ids.clone(),
                coll_handle: coll_handle.clone(),
            },
            Self { schedule, thread_ids, coll_handle: coll_handle.clone() },
        )
    }

    pub(crate) fn set_new_schedule(&mut self, schedule: Schedule, coll_handle: &basedrop::Handle) {
        self.schedule.set(Shared::new(coll_handle, AtomicRefCell::new(schedule)));
    }

    pub(crate) fn process_interleaved(
        &mut self,
        audio_in: &[f32],
        audio_in_channels: usize,
        audio_out: &mut [f32],
        audio_out_channels: usize,
    ) {
        let latest_schedule = self.schedule.get();

        let mut schedule = latest_schedule.borrow_mut();

        // TODO: Set this in the sandbox thread once we implement plugin sandboxing.
        // Make sure the the audio thread ID is correct.
        if let Some(audio_thread_id) = self.thread_ids.external_audio_thread_id() {
            if std::thread::current().id() != audio_thread_id {
                self.thread_ids
                    .set_external_audio_thread_id(std::thread::current().id(), &self.coll_handle);
            }
        } else {
            self.thread_ids
                .set_external_audio_thread_id(std::thread::current().id(), &self.coll_handle);
        }

        schedule.process_interleaved(audio_in, audio_in_channels, audio_out, audio_out_channels);
    }
}
