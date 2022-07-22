use atomic_refcell::{AtomicRef, AtomicRefCell, AtomicRefMut};
use basedrop::Shared;
use smallvec::SmallVec;
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};

pub use clack_host::events::io::{EventBuffer, EventBufferIter};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebugBufferType {
    Audio32,
    Audio64,
    IntermediaryAudio32,
    Event,
    Note,
}

impl Debug for DebugBufferType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DebugBufferType::Audio32 => f.write_str("f32"),
            DebugBufferType::Audio64 => f.write_str("f64"),
            DebugBufferType::IntermediaryAudio32 => f.write_str("intermediary_f32"),
            DebugBufferType::Event => f.write_str("event"),
            DebugBufferType::Note => f.write_str("note"),
        }
    }
}

/// Used for debugging and verifying purposes.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct DebugBufferID {
    pub index: u32,
    pub buffer_type: DebugBufferType,
}

impl Debug for DebugBufferID {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}({})", self.buffer_type, self.index)
    }
}

struct Buffer<T: Clone + Copy + Send + Sync + 'static> {
    data: AtomicRefCell<Vec<T>>,
    is_constant: AtomicBool,
    debug_info: DebugBufferID,
}

impl<T: Clone + Copy + Send + Sync + 'static> Buffer<T> {}

pub struct SharedBuffer<T: Clone + Copy + Send + Sync + 'static> {
    buffer: Shared<Buffer<T>>,
}

impl<T: Clone + Copy + Send + Sync + 'static> SharedBuffer<T> {
    pub fn with_capacity(
        capacity: usize,
        debug_info: DebugBufferID,
        coll_handle: &basedrop::Handle,
    ) -> Self {
        Self {
            buffer: Shared::new(
                coll_handle,
                Buffer {
                    data: AtomicRefCell::new(Vec::with_capacity(capacity)),
                    is_constant: AtomicBool::new(false),
                    debug_info,
                },
            ),
        }
    }

    #[inline]
    pub fn borrow(&self) -> AtomicRef<Vec<T>> {
        self.buffer.data.borrow()
    }

    #[inline]
    pub fn borrow_mut(&self) -> AtomicRefMut<Vec<T>> {
        self.buffer.data.borrow_mut()
    }

    #[inline]
    pub fn set_constant(&self, is_constant: bool) {
        self.buffer.is_constant.store(is_constant, Ordering::SeqCst);
    }

    #[inline]
    pub fn is_constant(&self) -> bool {
        self.buffer.is_constant.load(Ordering::SeqCst)
    }

    #[inline]
    pub fn id(&self) -> DebugBufferID {
        self.buffer.debug_info
    }

    pub fn truncate(&self) {
        self.borrow_mut().truncate(0)
    }
}

impl<T: Clone + Copy + Send + Sync + 'static + Default> SharedBuffer<T> {
    pub fn new(
        max_frames: usize,
        debug_info: DebugBufferID,
        coll_handle: &basedrop::Handle,
    ) -> Self {
        Self {
            buffer: Shared::new(
                coll_handle,
                Buffer {
                    data: AtomicRefCell::new(vec![T::default(); max_frames]),
                    is_constant: AtomicBool::new(false),
                    debug_info,
                },
            ),
        }
    }

    pub fn clear(&self) {
        self.borrow_mut().fill(T::default())
    }

    pub fn clear_until(&self, frames: usize) {
        let mut buf_ref = self.borrow_mut();
        let frames = frames.min(buf_ref.len());

        buf_ref[0..frames].fill(T::default());
    }
}

impl<T: Clone + Copy + Send + Sync + 'static> Clone for SharedBuffer<T> {
    fn clone(&self) -> Self {
        Self { buffer: Shared::clone(&self.buffer) }
    }
}

#[allow(unused)]
pub enum RawAudioChannelBuffers {
    F32(SmallVec<[SharedBuffer<f32>; 2]>),
    F64(SmallVec<[SharedBuffer<f64>; 2]>),
}

impl Debug for RawAudioChannelBuffers {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match &self {
            RawAudioChannelBuffers::F32(buffers) => {
                f.debug_list().entries(buffers.iter().map(|b| b.id())).finish()
            }
            RawAudioChannelBuffers::F64(buffers) => {
                f.debug_list().entries(buffers.iter().map(|b| b.id())).finish()
            }
        }
    }
}

pub struct AudioPortBuffer {
    pub _raw_channels: RawAudioChannelBuffers,
    latency: u32,
}

impl Debug for AudioPortBuffer {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        self._raw_channels.fmt(f)
    }
}

impl AudioPortBuffer {
    pub fn _new(buffers: SmallVec<[SharedBuffer<f32>; 2]>, latency: u32) -> Self {
        Self { _raw_channels: RawAudioChannelBuffers::F32(buffers), latency }
    }

    pub fn latency(&self) -> u32 {
        self.latency
    }

    pub fn is_silent(&self, frames: usize) -> bool {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                for buf in buffers.iter() {
                    let buf = buf.borrow();
                    let buf = &buf[0..frames.min(buf.len())];
                    for x in buf.iter() {
                        if *x != 0.0 {
                            return false;
                        }
                    }
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                for buf in buffers.iter() {
                    let buf = buf.borrow();
                    let buf = &buf[0..frames.min(buf.len())];
                    for x in buf.iter() {
                        if *x != 0.0 {
                            return false;
                        }
                    }
                }
            }
        }

        true
    }

    // TODO: Helper methods to retrieve more than 2 channels at once
}

pub struct AudioPortBufferMut {
    pub _raw_channels: RawAudioChannelBuffers,
    latency: u32,
}

impl Debug for AudioPortBufferMut {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self._raw_channels.fmt(f)
    }
}

impl AudioPortBufferMut {
    pub fn _new(buffers: SmallVec<[SharedBuffer<f32>; 2]>, latency: u32) -> Self {
        Self { _raw_channels: RawAudioChannelBuffers::F32(buffers), latency }
    }

    pub fn latency(&self) -> u32 {
        self.latency
    }

    #[inline]
    pub fn stereo_f32_mut(&mut self) -> Option<(AtomicRefMut<Vec<f32>>, AtomicRefMut<Vec<f32>>)> {
        match &mut self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                if b.len() > 1 {
                    Some((b[0].borrow_mut(), b[1].borrow_mut()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn is_silent(&self, frames: usize) -> bool {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                for buf in buffers {
                    let buf = buf.borrow();
                    let buf = &buf[0..frames.min(buf.len())];
                    for x in buf.iter() {
                        if *x != 0.0 {
                            return false;
                        }
                    }
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                for buf in buffers {
                    let buf = buf.borrow();
                    let buf = &buf[0..frames.min(buf.len())];
                    for x in buf.iter() {
                        if *x != 0.0 {
                            return false;
                        }
                    }
                }
            }
        }

        true
    }

    pub fn clear_all(&mut self, frames: usize) {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                for buf in buffers {
                    buf.clear_until(frames);
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                for buf in buffers {
                    buf.clear_until(frames);
                }
            }
        }
    }

    // TODO: Helper methods to retrieve more than 2 channels at once
}
