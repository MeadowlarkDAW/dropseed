use dropseed_core::plugin::buffer::{DebugBufferID, DebugBufferType, SharedBuffer};
use dropseed_core::plugin::ProcEvent;

struct BufferPool<T: Clone + Copy + Send + Sync + 'static> {
    pool: Vec<SharedBuffer<T>>,
    buffer_size: usize,
    collection_handle: basedrop::Handle,
}

impl<T: Clone + Copy + Send + Sync + 'static> BufferPool<T> {
    fn new(buffer_size: usize, collection_handle: basedrop::Handle) -> Self {
        Self { pool: Vec::new(), buffer_size, collection_handle }
    }

    fn get_new(&mut self) -> SharedBuffer<T> {}
}

pub struct SharedBufferPool {
    pub audio_buffer_size: u32,
    pub note_buffer_size: usize,
    pub automation_buffer_size: usize,

    audio_buffers_f32: Vec<Option<SharedBuffer<f32>>>,
    audio_buffers_f64: Vec<Option<SharedBuffer<f64>>>,

    intermediary_audio_f32: Vec<Option<SharedBuffer<f32>>>,

    automation_buffers: Vec<Option<SharedBuffer<ProcEvent>>>,
    note_buffers: Vec<Option<SharedBuffer<ProcEvent>>>,

    coll_handle: basedrop::Handle,
}

impl SharedBufferPool {
    pub fn new(
        audio_buffer_size: u32,
        note_buffer_size: usize,
        automation_buffer_size: usize,
        coll_handle: basedrop::Handle,
    ) -> Self {
        assert_ne!(audio_buffer_size, 0);
        assert_ne!(note_buffer_size, 0);
        assert_ne!(automation_buffer_size, 0);

        Self {
            audio_buffers_f32: Vec::new(),
            audio_buffers_f64: Vec::new(),
            intermediary_audio_f32: Vec::new(),
            automation_buffers: Vec::new(),
            note_buffers: Vec::new(),
            audio_buffer_size,
            note_buffer_size,
            automation_buffer_size,
            coll_handle,
        }
    }

    pub fn audio_f32(&mut self, index: usize) -> SharedBuffer<f32> {
        if self.audio_buffers_f32.len() <= index {
            let n_new_slots = (index + 1) - self.audio_buffers_f32.len();
            for _ in 0..n_new_slots {
                self.audio_buffers_f32.push(None);
            }
        }

        let slot = &mut self.audio_buffers_f32[index];

        if let Some(b) = slot {
            b.clone()
        } else {
            *slot = Some(SharedBuffer::_new_f32(
                self.audio_buffer_size as usize,
                DebugBufferID { buffer_type: DebugBufferType::Audio32, index: index as u32 },
                &self.coll_handle,
            ));

            slot.as_ref().unwrap().clone()
        }
    }

    pub fn intermediary_audio_f32(&mut self, index: usize) -> SharedBuffer<f32> {
        if self.intermediary_audio_f32.len() <= index {
            let n_new_slots = (index + 1) - self.intermediary_audio_f32.len();
            for _ in 0..n_new_slots {
                self.intermediary_audio_f32.push(None);
            }
        }

        let slot = &mut self.intermediary_audio_f32[index];

        if let Some(b) = slot {
            b.clone()
        } else {
            *slot = Some(SharedBuffer::_new_f32(
                self.audio_buffer_size as usize,
                DebugBufferID {
                    buffer_type: DebugBufferType::IntermediaryAudio32,
                    index: index as u32,
                },
                &self.coll_handle,
            ));

            slot.as_ref().unwrap().clone()
        }
    }

    pub fn note_buffer(&mut self, index: usize) -> SharedBuffer<ProcEvent> {
        if self.note_buffers.len() <= index {
            let n_new_slots = (index + 1) - self.note_buffers.len();
            for _ in 0..n_new_slots {
                self.note_buffers.push(None);
            }
        }

        let slot = &mut self.note_buffers[index];

        if let Some(b) = slot {
            b.clone()
        } else {
            *slot = Some(SharedBuffer::_new(
                self.note_buffer_size,
                DebugBufferID { buffer_type: DebugBufferType::NoteBuffer, index: index as u32 },
                &self.coll_handle,
            ));

            slot.as_ref().unwrap().clone()
        }
    }

    pub fn automation_buffer(&mut self, index: usize) -> SharedBuffer<ProcEvent> {
        if self.automation_buffers.len() <= index {
            let n_new_slots = (index + 1) - self.automation_buffers.len();
            for _ in 0..n_new_slots {
                self.automation_buffers.push(None);
            }
        }

        let slot = &mut self.automation_buffers[index];

        if let Some(b) = slot {
            b.clone()
        } else {
            *slot = Some(SharedBuffer::_new(
                self.automation_buffer_size,
                DebugBufferID {
                    buffer_type: DebugBufferType::ParamAutomation,
                    index: index as u32,
                },
                &self.coll_handle,
            ));

            slot.as_ref().unwrap().clone()
        }
    }

    pub fn remove_excess_buffers(
        &mut self,
        max_buffer_index: usize,
        total_intermediary_buffers: usize,
        max_note_buffer_index: usize,
        max_automation_buffer_index: usize,
    ) {
        if self.audio_buffers_f32.len() > max_buffer_index + 1 {
            let n_slots_to_remove = self.audio_buffers_f32.len() - (max_buffer_index + 1);
            for _ in 0..n_slots_to_remove {
                let _ = self.audio_buffers_f32.pop();
            }
        }
        if self.audio_buffers_f64.len() > max_buffer_index + 1 {
            let n_slots_to_remove = self.audio_buffers_f64.len() - (max_buffer_index + 1);
            for _ in 0..n_slots_to_remove {
                let _ = self.audio_buffers_f64.pop();
            }
        }
        if self.intermediary_audio_f32.len() > total_intermediary_buffers {
            let n_slots_to_remove = self.intermediary_audio_f32.len() - total_intermediary_buffers;
            for _ in 0..n_slots_to_remove {
                let _ = self.intermediary_audio_f32.pop();
            }
        }
        if self.note_buffers.len() > max_note_buffer_index + 1 {
            let n_slots_to_remove = self.note_buffers.len() - (max_note_buffer_index + 1);
            for _ in 0..n_slots_to_remove {
                let _ = self.note_buffers.pop();
            }
        }
        if self.automation_buffers.len() > max_automation_buffer_index + 1 {
            let n_slots_to_remove =
                self.automation_buffers.len() - (max_automation_buffer_index + 1);
            for _ in 0..n_slots_to_remove {
                let _ = self.automation_buffers.pop();
            }
        }
    }
}
