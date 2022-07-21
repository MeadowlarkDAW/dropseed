use crate::graph::buffers::events::{NoteEvent, NoteEventType, ParamEvent};
use crate::EventBuffer;
use clack_host::events::event_types::{NoteEvent as ClackNoteEvent, NoteOnEvent};
use dropseed_core::plugin::buffer::SharedBuffer;
use smallvec::SmallVec;

// TODO: remove pubs
pub struct PluginEventIoBuffers {
    pub unmixed_param_in_buffers: Option<SmallVec<[SharedBuffer<ParamEvent>; 2]>>,
    /// Only for internal plugin (e.g. timeline or macros)
    pub param_out_buffer: Option<SharedBuffer<ParamEvent>>,

    // TODO: remove options
    pub unmixed_note_in_buffers: SmallVec<[Option<SmallVec<[SharedBuffer<NoteEvent>; 2]>>; 2]>,
    pub note_out_buffers: SmallVec<[Option<SharedBuffer<NoteEvent>>; 2]>,
}

impl PluginEventIoBuffers {
    pub fn clear_before_process(&mut self) {
        if let Some(buffer) = &mut self.param_out_buffer {
            buffer.truncate();
        }

        for buffer in self.note_out_buffers.iter().flatten() {
            buffer.truncate();
        }
    }

    pub fn move_input_events_to(&self, raw_event_buffer: &mut EventBuffer) {
        let in_events = self
            .unmixed_note_in_buffers
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|e| (i, e)))
            .flat_map(|(i, b)| b.iter().map(|b| (i, b.borrow())));

        for (note_port_index, buffer) in in_events {
            for event in buffer.iter() {
                match event.event_type {
                    NoteEventType::On { velocity } => {
                        let e = ClackNoteEvent::<NoteOnEvent>::new();
                    }
                    NoteEventType::Expression { .. } => {}
                    NoteEventType::Choke => {}
                    NoteEventType::Off { .. } => {}
                }
            }
            todo!()
        }
    }
}
