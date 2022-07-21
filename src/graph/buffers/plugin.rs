use crate::graph::buffers::events::{NoteEvent, ParamEvent, PluginEvent};
use crate::graph::buffers::sanitization::PluginEventOutputSanitizer;
use crate::graph::plugin_host::{AudioToMainParamValue, ParamQueuesAudioThread};
use crate::utils::reducing_queue::ReducFnvProducerRefMut;
use crate::EventBuffer;
use clack_host::utils::Cookie;
use dropseed_core::plugin::buffer::SharedBuffer;
use dropseed_core::plugin::ParamID;
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

    pub fn write_input_events(&self, raw_event_buffer: &mut EventBuffer) -> (bool, bool) {
        let wrote_note_event = self.write_input_note_events(raw_event_buffer);
        let wrote_param_event = self.write_input_param_events(raw_event_buffer);

        (wrote_note_event, wrote_param_event)
    }

    fn write_input_note_events(&self, raw_event_buffer: &mut EventBuffer) -> bool {
        // TODO: make this clearer
        let in_events = self
            .unmixed_note_in_buffers
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|e| (i, e)))
            .flat_map(|(i, b)| b.iter().map(|b| (i, b.borrow())));

        let mut wrote_note_event = false;

        for (note_port_index, buffer) in in_events {
            for event in buffer.iter() {
                let event = PluginEvent::NoteEvent {
                    note_port_index: note_port_index as i16,
                    event: *event,
                };
                event.write_to_clap_buffer(raw_event_buffer);
                wrote_note_event = true;
            }
        }

        wrote_note_event
    }

    fn write_input_param_events(&self, raw_event_buffer: &mut EventBuffer) -> bool {
        let mut wrote_param_event = false;
        for in_buf in self.unmixed_param_in_buffers.iter().flatten() {
            for event in in_buf.borrow().iter() {
                // TODO: handle cookies?
                let event = PluginEvent::ParamEvent { cookie: Cookie::empty(), event: *event };
                event.write_to_clap_buffer(raw_event_buffer);
                wrote_param_event = true;
            }
        }
        wrote_param_event
    }

    pub fn read_output_events(
        &mut self,
        raw_event_buffer: &EventBuffer,
        external_parameter_queue: Option<
            &mut ReducFnvProducerRefMut<ParamID, AudioToMainParamValue>,
        >,
        sanitizer: &mut PluginEventOutputSanitizer,
        param_target_plugin_id: u64,
    ) {
        let events_iter = raw_event_buffer
            .iter()
            .filter_map(|e| PluginEvent::read_from_clap(e, param_target_plugin_id));
        let events_iter = sanitizer.sanitize(events_iter);

        for event in events_iter {
            match event {
                PluginEvent::NoteEvent { note_port_index, event } => {
                    if let Some(Some(b)) = self.note_out_buffers.get(note_port_index as usize) {
                        b.borrow_mut().push(event)
                    }
                }
                PluginEvent::ParamEvent { cookie: _, event } => {
                    if let Some(buffer) = &mut self.param_out_buffer {
                        buffer.borrow().push(event)
                    }

                    if let Some(queue) = external_parameter_queue {
                        if let Some(value) =
                            AudioToMainParamValue::from_param_event(event.event_type)
                        {
                            queue.set_or_update(ParamID::new(event.parameter_id), value);
                        }
                    }
                }
            }
        }
    }
}
