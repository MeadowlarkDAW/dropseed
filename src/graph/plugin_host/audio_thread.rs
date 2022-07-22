use crate::graph::buffers::plugin::PluginEventIoBuffers;
use crate::graph::buffers::sanitization::PluginEventOutputSanitizer;
use crate::graph::plugin_host::{ParamQueuesAudioThread, PluginState, SharedPluginState};
use clack_host::events::Event;
use dropseed_core::plugin::buffer::EventBuffer;
use dropseed_core::plugin::{
    PluginAudioThread, PluginInstanceID, ProcBuffers, ProcInfo, ProcessStatus,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Copy, Clone, Debug, PartialEq)]
enum ProcessingState {
    WaitingForStart,
    Started(ProcessStatus),
    Stopped,
    Errored,
}

pub(crate) struct PluginInstanceHostAudioThread {
    pub id: PluginInstanceID,

    plugin: Box<dyn PluginAudioThread>,

    state: Arc<SharedPluginState>,

    param_queues: Option<ParamQueuesAudioThread>,
    in_events: EventBuffer,
    out_events: EventBuffer,

    event_output_sanitizer: PluginEventOutputSanitizer,

    processing_state: ProcessingState,
}

impl PluginInstanceHostAudioThread {
    pub(crate) fn new(
        id: PluginInstanceID,
        plugin: Box<dyn PluginAudioThread>,
        state: Arc<SharedPluginState>,
        param_queues: Option<ParamQueuesAudioThread>,
        num_params: usize,
    ) -> Self {
        Self {
            id,
            plugin,
            state,
            param_queues,
            in_events: EventBuffer::with_capacity(num_params * 3),
            out_events: EventBuffer::with_capacity(num_params * 3),
            event_output_sanitizer: PluginEventOutputSanitizer::new(num_params),
            processing_state: ProcessingState::WaitingForStart,
        }
    }

    pub fn process(
        &mut self,
        proc_info: &ProcInfo,
        buffers: &mut ProcBuffers,
        event_buffers: &mut PluginEventIoBuffers,
    ) {
        // Always clear event and note output buffers.
        event_buffers.clear_before_process();

        let state = self.state.get_state();

        // Do we want to deactivate the plugin?
        if state == PluginState::WaitingToDrop {
            if let ProcessingState::Started(_) = self.processing_state {
                self.plugin.stop_processing();
            }

            buffers.clear_all_outputs(proc_info);
            return;
        } else if self.state.start_processing.load(Ordering::Relaxed) {
            self.state.start_processing.store(false, Ordering::Relaxed);

            if let ProcessingState::Started(_) = self.processing_state {
            } else {
                self.processing_state = ProcessingState::WaitingForStart;
            }
        }

        // We can't process a plugin which failed to start processing.
        if self.processing_state == ProcessingState::Errored {
            buffers.clear_all_outputs(proc_info);
            return;
        }

        // Reading in_events from all sources //

        self.in_events.clear();
        let mut has_param_in_event = self
            .param_queues
            .as_mut()
            .map(|q| q.consume_into_event_buffer(&mut self.in_events))
            .unwrap_or(false);

        let (has_note_in_event, wrote_param_in_event) =
            event_buffers.write_input_events(&mut self.in_events);

        has_param_in_event = has_param_in_event || wrote_param_in_event;

        // //

        if let Some(transport_in_event) = proc_info.transport.event() {
            self.in_events.push(transport_in_event.as_unknown());
        }

        if self.processing_state == ProcessingState::Started(ProcessStatus::ContinueIfNotQuiet)
            && !has_note_in_event
        {
            if buffers.audio_inputs_silent(proc_info.frames) {
                self.plugin.stop_processing();

                self.processing_state = ProcessingState::Stopped;
                buffers.clear_all_outputs(proc_info);

                if has_param_in_event {
                    self.plugin.param_flush(&self.in_events, &mut self.out_events);
                }

                self.in_events.clear();
                return;
            }
        }

        if let ProcessingState::Stopped | ProcessingState::WaitingForStart = self.processing_state {
            if self.processing_state == ProcessingState::Stopped && !has_note_in_event {
                // The plugin is sleeping, there is no request to wake it up, and there
                // are no events to process.
                buffers.clear_all_outputs(proc_info);

                if has_param_in_event {
                    self.plugin.param_flush(&self.in_events, &mut self.out_events);
                }

                self.in_events.clear();
                return;
            }

            if self.plugin.start_processing().is_err() {
                // The plugin failed to start processing.
                self.processing_state = ProcessingState::Errored;
                buffers.clear_all_outputs(proc_info);

                if has_param_in_event {
                    self.plugin.param_flush(&self.in_events, &mut self.out_events);
                }

                return;
            }

            self.state.set_state(PluginState::Active);
        }

        self.out_events.clear();
        let new_status =
            self.plugin.process(proc_info, buffers, &self.in_events, &mut self.out_events);

        if let Some(params_queue) = &mut self.param_queues {
            params_queue.audio_to_main_param_value_tx.produce(|mut producer| {
                event_buffers.read_output_events(
                    &self.out_events,
                    Some(&mut producer),
                    &mut self.event_output_sanitizer,
                    0, // TODO: find right plugin instance ID value
                )
            });
        } else {
            event_buffers.read_output_events(
                &self.out_events,
                None,
                &mut self.event_output_sanitizer,
                0, // TODO: find right plugin instance ID value
            );
        }

        self.processing_state = match new_status {
            // ProcessStatus::Tail => TODO: handle tail by reading from the tail extension
            ProcessStatus::Sleep => {
                self.plugin.stop_processing();

                ProcessingState::Stopped
            }
            ProcessStatus::Error => {
                // Discard all output buffers.
                buffers.clear_all_outputs(proc_info);
                ProcessingState::Errored
            }
            good_status => ProcessingState::Started(good_status),
        };
    }
}

impl Drop for PluginInstanceHostAudioThread {
    fn drop(&mut self) {
        if let ProcessingState::Started(_) = self.processing_state {
            self.plugin.stop_processing();
        }

        self.state.set_state(PluginState::DroppedAndReadyToDeactivate);
    }
}
