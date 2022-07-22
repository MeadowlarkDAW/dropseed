use crate::graph::buffers::events::{NoteEvent, ParamEvent};
use dropseed_core::plugin::buffer::{DebugBufferID, DebugBufferType, SharedBuffer};

pub struct BufferPool<T: Clone + Copy + Send + Sync + 'static> {
    pool: Vec<SharedBuffer<T>>,
    buffer_size: usize,
    buffer_type: DebugBufferType,
    collection_handle: basedrop::Handle,
}

impl<T: Clone + Copy + Send + Sync + 'static> BufferPool<T> {
    fn new(
        buffer_size: usize,
        buffer_type: DebugBufferType,
        collection_handle: basedrop::Handle,
    ) -> Self {
        assert_ne!(buffer_size, 0);

        Self { pool: Vec::new(), buffer_size, collection_handle, buffer_type }
    }

    #[inline]
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    pub fn buffer_at_index(&mut self, index: usize) -> SharedBuffer<T> {
        if index >= self.pool.len() {
            let mut current_generated_index = self.pool.len() as u32;

            self.pool.resize_with(index + 1, || {
                let buf = SharedBuffer::with_capacity(
                    self.buffer_size,
                    DebugBufferID { index: current_generated_index, buffer_type: self.buffer_type },
                    &self.collection_handle,
                );
                current_generated_index += 1;
                buf
            })
        }

        // PANIC: we have checked above that either index was in-bounds, or the pool got resized
        self.pool[index].clone()
    }
}

impl<T: Clone + Copy + Send + Sync + 'static + Default> BufferPool<T> {
    pub fn initialized_buffer_at_index(&mut self, index: usize) -> SharedBuffer<T> {
        if index >= self.pool.len() {
            let mut current_generated_index = self.pool.len() as u32;

            self.pool.resize_with(index + 1, || {
                let buf = SharedBuffer::new(
                    self.buffer_size,
                    DebugBufferID { index: current_generated_index, buffer_type: self.buffer_type },
                    &self.collection_handle,
                );
                current_generated_index += 1;
                buf
            })
        }

        // PANIC: we have checked above that either index was in-bounds, or the pool got resized
        self.pool[index].clone()
    }
}

pub struct SharedBufferPool {
    pub audio_buffer_pool: BufferPool<f32>,
    pub intermediary_audio_buffer_pool: BufferPool<f32>,

    pub param_event_buffer_pool: BufferPool<ParamEvent>,
    pub note_buffer_pool: BufferPool<NoteEvent>,
}

impl SharedBufferPool {
    pub fn new(
        audio_buffer_size: usize,
        note_buffer_size: usize,
        event_buffer_size: usize,
        coll_handle: basedrop::Handle,
    ) -> Self {
        Self {
            audio_buffer_pool: BufferPool::new(
                audio_buffer_size,
                DebugBufferType::Audio32,
                coll_handle.clone(),
            ),
            intermediary_audio_buffer_pool: BufferPool::new(
                audio_buffer_size,
                DebugBufferType::IntermediaryAudio32,
                coll_handle.clone(),
            ),
            note_buffer_pool: BufferPool::new(
                note_buffer_size,
                DebugBufferType::Note,
                coll_handle.clone(),
            ),
            param_event_buffer_pool: BufferPool::new(
                event_buffer_size,
                DebugBufferType::Event,
                coll_handle,
            ),
        }
    }

    pub fn remove_excess_buffers(
        &mut self,
        audio_buffer_count: usize,
        intermediary_audio_buffer_count: usize,
        note_buffers_count: usize,
        event_buffer_count: usize,
    ) {
        self.audio_buffer_pool.pool.truncate(audio_buffer_count + 1);
        self.intermediary_audio_buffer_pool.pool.truncate(intermediary_audio_buffer_count + 1);
        self.note_buffer_pool.pool.truncate(note_buffers_count + 1);
        self.param_event_buffer_pool.pool.truncate(event_buffer_count + 1);
    }
}
