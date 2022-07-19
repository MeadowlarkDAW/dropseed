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
pub struct DebugBufferInfo {
    pub index: u32,
    pub buffer_type: DebugBufferType,
}

impl Debug for DebugBufferInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}({})", self.buffer_type, self.index)
    }
}

struct Buffer<T: Clone + Copy + Send + Sync + 'static> {
    data: AtomicRefCell<Vec<T>>,
    is_constant: AtomicBool,
    debug_info: DebugBufferInfo,
}

impl<T: Clone + Copy + Send + Sync + 'static> Buffer<T> {}

pub struct SharedBuffer<T: Clone + Copy + Send + Sync + 'static> {
    buffer: Shared<Buffer<T>>,
}

impl<T: Clone + Copy + Send + Sync + 'static> SharedBuffer<T> {
    pub fn with_capacity(
        capacity: usize,
        debug_info: DebugBufferInfo,
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
    pub fn info(&self) -> DebugBufferInfo {
        self.buffer.debug_info
    }

    pub fn truncate(&self) {
        self.borrow_mut().truncate(0)
    }
}

impl<T: Clone + Copy + Send + Sync + 'static + Default> SharedBuffer<T> {
    pub fn new(
        max_frames: usize,
        debug_info: DebugBufferInfo,
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

        if frames >= buf_ref.len() {
            self.set_constant(true);
        }
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

impl RawAudioChannelBuffers {
    fn num_channels(&self) -> usize {
        match self {
            RawAudioChannelBuffers::F32(c) => c.len(),
            RawAudioChannelBuffers::F64(c) => c.len(),
        }
    }
}

impl Debug for RawAudioChannelBuffers {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match &self {
            RawAudioChannelBuffers::F32(buffers) => {
                f.debug_list().entries(buffers.iter().map(|b| b.info())).finish()
            }
            RawAudioChannelBuffers::F64(buffers) => {
                f.debug_list().entries(buffers.iter().map(|b| b.info())).finish()
            }
        }
    }
}

pub enum MonoBuffer<'a> {
    F32(AtomicRef<'a, Vec<f32>>),
    F64(AtomicRef<'a, Vec<f64>>),
}

pub enum MonoBufferMut<'a> {
    F32(AtomicRefMut<'a, Vec<f32>>),
    F64(AtomicRefMut<'a, Vec<f64>>),
}

pub enum StereoBuffer<'a> {
    F32(AtomicRef<'a, Vec<f32>>, AtomicRef<'a, Vec<f32>>),
    F64(AtomicRef<'a, Vec<f64>>, AtomicRef<'a, Vec<f64>>),
}

pub enum StereoBufferMut<'a> {
    F32(AtomicRefMut<'a, Vec<f32>>, AtomicRefMut<'a, Vec<f32>>),
    F64(AtomicRefMut<'a, Vec<f64>>, AtomicRefMut<'a, Vec<f64>>),
}

pub struct AudioPortBuffer {
    pub _raw_channels: RawAudioChannelBuffers,

    latency: u32,

    constant_mask: u64,
}

impl Debug for AudioPortBuffer {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        self._raw_channels.fmt(f)
    }
}

impl AudioPortBuffer {
    pub fn _new(buffers: SmallVec<[SharedBuffer<f32>; 2]>, latency: u32) -> Self {
        Self { _raw_channels: RawAudioChannelBuffers::F32(buffers), latency, constant_mask: 0 }
    }

    pub fn _sync_constant_mask_from_buffers(&mut self) {
        self.constant_mask = 0;

        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    if buf.is_constant() {
                        self.constant_mask |= 1 << i;
                    }
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    if buf.is_constant() {
                        self.constant_mask |= 1 << i;
                    }
                }
            }
        }
    }

    pub fn num_channels(&self) -> usize {
        self._raw_channels.num_channels()
    }

    pub fn latency(&self) -> u32 {
        self.latency
    }

    pub fn constant_mask(&self) -> u64 {
        self.constant_mask
    }

    #[inline]
    pub fn channel(&self, index: usize) -> Option<MonoBuffer> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                b.get(index).map(|b| MonoBuffer::F32(b.buffer.data.borrow()))
            }
            RawAudioChannelBuffers::F64(b) => {
                b.get(index).map(|b| MonoBuffer::F64(b.buffer.data.borrow()))
            }
        }
    }

    #[inline]
    pub fn mono(&self) -> MonoBuffer {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => MonoBuffer::F32(b[0].buffer.data.borrow()),
            RawAudioChannelBuffers::F64(b) => MonoBuffer::F64(b[0].buffer.data.borrow()),
        }
    }

    #[inline]
    pub fn stereo(&self) -> Option<StereoBuffer> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                if b.len() > 1 {
                    Some(StereoBuffer::F32(b[0].buffer.data.borrow(), b[1].buffer.data.borrow()))
                } else {
                    None
                }
            }
            RawAudioChannelBuffers::F64(b) => {
                if b.len() > 1 {
                    Some(StereoBuffer::F64(b[0].buffer.data.borrow(), b[1].buffer.data.borrow()))
                } else {
                    None
                }
            }
        }
    }

    #[inline]
    pub fn channel_f32<'a>(&'a self, index: usize) -> Option<AtomicRef<'a, Vec<f32>>> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => b.get(index).map(|b| b.buffer.data.borrow()),
            _ => None,
        }
    }

    #[inline]
    pub fn mono_f32<'a>(&'a self) -> Option<AtomicRef<'a, Vec<f32>>> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => Some(b[0].buffer.data.borrow()),
            _ => None,
        }
    }

    #[inline]
    pub fn stereo_f32<'a>(&'a self) -> Option<(AtomicRef<'a, Vec<f32>>, AtomicRef<'a, Vec<f32>>)> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                if b.len() > 1 {
                    Some((b[0].buffer.data.borrow(), b[1].buffer.data.borrow()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn is_channel_silent(&self, ch: usize, use_slow_check: bool, frames: usize) -> bool {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                if let Some(buf) = buffers.get(ch) {
                    if self.constant_mask & (1 << ch) != 0 {
                        let buf = buf.borrow();
                        if buf[0] != 0.0 {
                            return false;
                        }
                    } else if use_slow_check {
                        let buf = buf.borrow();
                        let buf = &buf[0..frames.min(buf.len())];
                        for x in buf.iter() {
                            if *x != 0.0 {
                                return false;
                            }
                        }
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                if let Some(buf) = buffers.get(ch) {
                    if self.constant_mask & (1 << ch) != 0 {
                        let buf = buf.borrow();
                        if buf[0] != 0.0 {
                            return false;
                        }
                    } else if use_slow_check {
                        let buf = buf.borrow();
                        let buf = &buf[0..frames.min(buf.len())];
                        for x in buf.iter() {
                            if *x != 0.0 {
                                return false;
                            }
                        }
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }

        true
    }

    pub fn is_silent(&self, use_slow_check: bool, frames: usize) -> bool {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    if self.constant_mask & (1 << i) != 0 {
                        let buf = buf.borrow();
                        if buf[0] != 0.0 {
                            return false;
                        }
                    } else if use_slow_check {
                        let buf = buf.borrow();
                        let buf = &buf[0..frames.min(buf.len())];
                        for x in buf.iter() {
                            if *x != 0.0 {
                                return false;
                            }
                        }
                    } else {
                        return false;
                    }
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    if self.constant_mask & (1 << i) != 0 {
                        let buf = buf.borrow();
                        if buf[0] != 0.0 {
                            return false;
                        }
                    } else if use_slow_check {
                        let buf = buf.borrow();
                        let buf = &buf[0..frames.min(buf.len())];
                        for x in buf.iter() {
                            if *x != 0.0 {
                                return false;
                            }
                        }
                    } else {
                        return false;
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
    pub constant_mask: u64,
}

impl Debug for AudioPortBufferMut {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self._raw_channels.fmt(f)
    }
}

impl AudioPortBufferMut {
    pub fn _new(buffers: SmallVec<[SharedBuffer<f32>; 2]>, latency: u32) -> Self {
        Self { _raw_channels: RawAudioChannelBuffers::F32(buffers), latency, constant_mask: 0 }
    }

    pub fn _sync_constant_mask_to_buffers(&mut self) {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    buf.set_constant(self.constant_mask & (1 << i) == 1);
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    buf.set_constant(self.constant_mask & (1 << i) == 1);
                }
            }
        }
    }

    pub fn num_channels(&self) -> usize {
        self._raw_channels.num_channels()
    }

    pub fn latency(&self) -> u32 {
        self.latency
    }

    #[inline]
    pub fn channel(&self, index: usize) -> Option<MonoBuffer> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                b.get(index).map(|b| MonoBuffer::F32(b.buffer.data.borrow()))
            }
            RawAudioChannelBuffers::F64(b) => {
                b.get(index).map(|b| MonoBuffer::F64(b.buffer.data.borrow()))
            }
        }
    }

    #[inline]
    pub fn channel_mut(&mut self, index: usize) -> Option<MonoBufferMut> {
        match &mut self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                b.get(index).map(|b| MonoBufferMut::F32(b.buffer.data.borrow_mut()))
            }
            RawAudioChannelBuffers::F64(b) => {
                b.get(index).map(|b| MonoBufferMut::F64(b.buffer.data.borrow_mut()))
            }
        }
    }

    #[inline]
    pub fn mono(&self) -> MonoBuffer {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => MonoBuffer::F32(b[0].buffer.data.borrow()),
            RawAudioChannelBuffers::F64(b) => MonoBuffer::F64(b[0].buffer.data.borrow()),
        }
    }

    #[inline]
    pub fn mono_mut(&mut self) -> MonoBufferMut {
        match &mut self._raw_channels {
            RawAudioChannelBuffers::F32(b) => MonoBufferMut::F32(b[0].buffer.data.borrow_mut()),
            RawAudioChannelBuffers::F64(b) => MonoBufferMut::F64(b[0].buffer.data.borrow_mut()),
        }
    }

    #[inline]
    pub fn stereo(&self) -> Option<StereoBuffer> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                if b.len() > 1 {
                    Some(StereoBuffer::F32(b[0].buffer.data.borrow(), b[1].buffer.data.borrow()))
                } else {
                    None
                }
            }
            RawAudioChannelBuffers::F64(b) => {
                if b.len() > 1 {
                    Some(StereoBuffer::F64(b[0].buffer.data.borrow(), b[1].buffer.data.borrow()))
                } else {
                    None
                }
            }
        }
    }

    #[inline]
    pub fn stereo_mut(&mut self) -> Option<StereoBufferMut> {
        match &mut self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                if b.len() > 1 {
                    Some(StereoBufferMut::F32(
                        b[0].buffer.data.borrow_mut(),
                        b[1].buffer.data.borrow_mut(),
                    ))
                } else {
                    None
                }
            }
            RawAudioChannelBuffers::F64(b) => {
                if b.len() > 1 {
                    Some(StereoBufferMut::F64(
                        b[0].buffer.data.borrow_mut(),
                        b[1].buffer.data.borrow_mut(),
                    ))
                } else {
                    None
                }
            }
        }
    }

    #[inline]
    pub fn channel_f32<'a>(&'a self, index: usize) -> Option<AtomicRef<'a, Vec<f32>>> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => b.get(index).map(|b| b.buffer.data.borrow()),
            _ => None,
        }
    }

    #[inline]
    pub fn channel_f32_mut<'a>(&'a mut self, index: usize) -> Option<AtomicRefMut<'a, Vec<f32>>> {
        match &mut self._raw_channels {
            RawAudioChannelBuffers::F32(b) => b.get_mut(index).map(|b| b.buffer.data.borrow_mut()),
            _ => None,
        }
    }

    #[inline]
    pub fn mono_f32<'a>(&'a self) -> Option<AtomicRef<'a, Vec<f32>>> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => Some(b[0].buffer.data.borrow()),
            _ => None,
        }
    }

    #[inline]
    pub fn mono_f32_mut<'a>(&'a mut self) -> Option<AtomicRefMut<'a, Vec<f32>>> {
        match &mut self._raw_channels {
            RawAudioChannelBuffers::F32(b) => Some(b[0].buffer.data.borrow_mut()),
            _ => None,
        }
    }

    #[inline]
    pub fn stereo_f32<'a>(&'a self) -> Option<(AtomicRef<'a, Vec<f32>>, AtomicRef<'a, Vec<f32>>)> {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                if b.len() > 1 {
                    Some((b[0].buffer.data.borrow(), b[1].buffer.data.borrow()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    #[inline]
    pub fn stereo_f32_mut<'a>(
        &'a mut self,
    ) -> Option<(AtomicRefMut<'a, Vec<f32>>, AtomicRefMut<'a, Vec<f32>>)> {
        match &mut self._raw_channels {
            RawAudioChannelBuffers::F32(b) => {
                if b.len() > 1 {
                    Some((b[0].buffer.data.borrow_mut(), b[1].buffer.data.borrow_mut()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn is_channel_silent(&self, ch: usize, use_slow_check: bool, frames: usize) -> bool {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                if let Some(buf) = buffers.get(ch) {
                    if self.constant_mask & (1 << ch) != 0 {
                        let buf = buf.borrow();
                        if buf[0] != 0.0 {
                            return false;
                        }
                    } else if use_slow_check {
                        let buf = buf.borrow();
                        let buf = &buf[0..frames.min(buf.len())];
                        for x in buf.iter() {
                            if *x != 0.0 {
                                return false;
                            }
                        }
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                if let Some(buf) = buffers.get(ch) {
                    if self.constant_mask & (1 << ch) != 0 {
                        let buf = buf.borrow();
                        if buf[0] != 0.0 {
                            return false;
                        }
                    } else if use_slow_check {
                        let buf = buf.borrow();
                        let buf = &buf[0..frames.min(buf.len())];
                        for x in buf.iter() {
                            if *x != 0.0 {
                                return false;
                            }
                        }
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }

        true
    }

    pub fn is_silent(&self, use_slow_check: bool, frames: usize) -> bool {
        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    if self.constant_mask & (1 << i) != 0 {
                        let buf = buf.borrow();
                        if buf[0] != 0.0 {
                            return false;
                        }
                    } else if use_slow_check {
                        let buf = buf.borrow();
                        let buf = &buf[0..frames.min(buf.len())];
                        for x in buf.iter() {
                            if *x != 0.0 {
                                return false;
                            }
                        }
                    } else {
                        return false;
                    }
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    if self.constant_mask & (1 << i) != 0 {
                        let buf = buf.borrow();
                        if buf[0] != 0.0 {
                            return false;
                        }
                    } else if use_slow_check {
                        let buf = buf.borrow();
                        let buf = &buf[0..frames.min(buf.len())];
                        for x in buf.iter() {
                            if *x != 0.0 {
                                return false;
                            }
                        }
                    } else {
                        return false;
                    }
                }
            }
        }

        true
    }

    pub fn clear_all(&mut self, frames: usize) {
        self.constant_mask = 0;

        match &self._raw_channels {
            RawAudioChannelBuffers::F32(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    self.constant_mask |= 1 << i;
                    let mut buf = buf.borrow_mut();
                    buf[0..frames].fill(0.0);
                }
            }
            RawAudioChannelBuffers::F64(buffers) => {
                for (i, buf) in buffers.iter().enumerate() {
                    self.constant_mask |= 1 << i;
                    let mut buf = buf.borrow_mut();
                    buf[0..frames].fill(0.0);
                }
            }
        }
    }

    // TODO: Helper methods to retrieve more than 2 channels at once
}
