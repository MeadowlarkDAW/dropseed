use basedrop::Shared;
use clack_host::events::event_types::{ParamModEvent, ParamValueEvent};
use clack_host::events::io::EventBuffer;
use clack_host::events::{Event, EventFlags, EventHeader};
use clack_host::utils::Cookie;
use crossbeam_channel::Sender;
use fnv::FnvHashMap;
use meadowlark_core_types::time::SampleRate;
use smallvec::SmallVec;
use std::error::Error;
use std::fmt::Debug;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};

use crate::graph::buffers::events::ParamEventType;
use crate::graph::buffers::plugin::PluginEventIoBuffers;
use crate::graph::buffers::sanitization::PluginEventOutputSanitizer;
use crate::{DSEngineEvent, PluginEvent};
use dropseed_core::plugin::ext::audio_ports::PluginAudioPortsExt;
use dropseed_core::plugin::ext::note_ports::PluginNotePortsExt;
use dropseed_core::plugin::ext::params::{ParamID, ParamInfo, ParamInfoFlags};
use dropseed_core::plugin::{
    HostRequestChannelReceiver, HostRequestFlags, PluginAudioThread, PluginInstanceID,
    PluginMainThread, PluginPreset, PluginSaveState, ProcBuffers, ProcInfo, ProcessStatus,
};
use dropseed_core::plugin_scanner::{PluginFormat, ScannedPluginKey};
use dropseed_core::transport::TempoMap;

use crate::graph::shared_pool::SharedPluginHostAudioThread;
use crate::utils::reducing_queue::{
    ReducFnvConsumer, ReducFnvProducer, ReducFnvValue, ReducingFnvQueue,
};

#[derive(Clone, Copy)]
struct MainToAudioParamValue {
    value: f64,
}

impl ReducFnvValue for MainToAudioParamValue {}

#[derive(Debug, Clone, Copy)]
pub struct ParamGestureInfo {
    pub is_begin: bool,
}

#[derive(Clone, Copy)]
pub struct AudioToMainParamValue {
    value: Option<f64>,
    gesture: Option<ParamGestureInfo>,
}

impl ReducFnvValue for AudioToMainParamValue {
    fn update(&mut self, new_value: &Self) {
        if new_value.value.is_some() {
            self.value = new_value.value;
        }

        if new_value.gesture.is_some() {
            self.gesture = new_value.gesture;
        }
    }
}

impl AudioToMainParamValue {
    pub fn from_param_event(event: ParamEventType) -> Option<Self> {
        match event {
            ParamEventType::Value(value) => Some(Self { value: Some(value), gesture: None }),
            ParamEventType::Modulation(_) => None, // TODO: handle mod events
            ParamEventType::BeginGesture => {
                Some(Self { value: None, gesture: Some(ParamGestureInfo { is_begin: true }) })
            }
            ParamEventType::EndGesture => {
                Some(Self { value: None, gesture: Some(ParamGestureInfo { is_begin: false }) })
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ParamModifiedInfo {
    pub param_id: ParamID,
    pub new_value: Option<f64>,
    pub is_gesturing: bool,
}

#[derive(Debug)]
pub struct PluginHandle {
    pub params: PluginParamsExt,
    pub internal: Option<Box<dyn std::any::Any + Send + 'static>>,
    pub(crate) audio_ports: PluginAudioPortsExt,
    pub(crate) note_ports: PluginNotePortsExt,
    pub(crate) has_automation_out_port: bool,
}

impl PluginHandle {
    pub fn audio_ports(&self) -> &PluginAudioPortsExt {
        &self.audio_ports
    }

    pub fn note_ports(&self) -> &PluginNotePortsExt {
        &self.note_ports
    }

    /// This will only return `true` for internal plugins which send parameter
    /// automation events to other plugins.
    ///
    /// Note, plugins always have an "automation in port".
    pub fn has_automation_out_port(&self) -> bool {
        self.has_automation_out_port
    }
}

pub struct PluginParamsExt {
    /// (parameter info, initial value)
    pub params: FnvHashMap<ParamID, ParamInfo>,

    ui_to_audio_param_value_tx: Option<ReducFnvProducer<ParamID, MainToAudioParamValue>>,
    ui_to_audio_param_mod_tx: Option<ReducFnvProducer<ParamID, MainToAudioParamValue>>,
}

impl Debug for PluginParamsExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("PluginParamsExt");
        f.field("params", &self.params);
        f.finish()
    }
}

impl PluginParamsExt {
    pub fn set_value(&mut self, param_id: ParamID, value: f64) {
        if let Some(ui_to_audio_param_value_tx) = &mut self.ui_to_audio_param_value_tx {
            if let Some(param_info) = self.params.get(&param_id) {
                if param_info.flags.contains(ParamInfoFlags::IS_READONLY) {
                    log::warn!("Ignored request to set parameter value: parameter with id {:?} is read only", &param_id);
                } else {
                    ui_to_audio_param_value_tx.set(param_id, MainToAudioParamValue { value });
                    ui_to_audio_param_value_tx.producer_done();
                }
            } else {
                log::warn!(
                    "Ignored request to set parameter value: plugin has no parameter with id {:?}",
                    &param_id
                );
            }
        } else {
            log::warn!("Ignored request to set parameter value: plugin has no parameters");
        }
    }

    pub fn set_mod_amount(&mut self, param_id: ParamID, amount: f64) {
        if let Some(ui_to_audio_param_mod_tx) = &mut self.ui_to_audio_param_mod_tx {
            if let Some(param_info) = self.params.get(&param_id) {
                ui_to_audio_param_mod_tx.set(param_id, MainToAudioParamValue { value: amount });
                ui_to_audio_param_mod_tx.producer_done();
            } else {
                log::warn!(
                    "Ignored request to set parameter mod amount: plugin has no parameter with id {:?}",
                    &param_id
                );
            }
        } else {
            log::warn!("Ignored request to set parameter mod amount: plugin has no parameters");
        }
    }
}

struct ParamQueuesMainThread {
    audio_to_main_param_value_rx: ReducFnvConsumer<ParamID, AudioToMainParamValue>,
}

pub(crate) struct ParamQueuesAudioThread {
    pub(crate) audio_to_main_param_value_tx: ReducFnvProducer<ParamID, AudioToMainParamValue>,

    ui_to_audio_param_value_rx: ReducFnvConsumer<ParamID, MainToAudioParamValue>,
    ui_to_audio_param_mod_rx: ReducFnvConsumer<ParamID, MainToAudioParamValue>,
}

impl ParamQueuesAudioThread {
    fn consume_into_event_buffer(&mut self, buffer: &mut EventBuffer) -> bool {
        let mut has_param_in_event = false;
        self.ui_to_audio_param_value_rx.consume(|param_id, value| {
            has_param_in_event = true;

            let event = ParamValueEvent::new(
                // TODO: Finer values for `time` instead of just setting it to the first frame?
                EventHeader::new_core(0, EventFlags::empty()),
                Cookie::empty(),
                // TODO: Note ID
                -1,                // note_id
                param_id.as_u32(), // param_id
                // TODO: Port index
                -1, // port_index
                // TODO: Channel
                -1, // channel
                // TODO: Key
                -1,          // key
                value.value, // value
            );

            buffer.push(event.as_unknown())
        });

        self.ui_to_audio_param_mod_rx.consume(|param_id, value| {
            has_param_in_event = true;

            let event = ParamModEvent::new(
                // TODO: Finer values for `time` instead of just setting it to the first frame?
                EventHeader::new_core(0, EventFlags::empty()),
                Cookie::empty(),
                // TODO: Note ID
                -1,                // note_id
                param_id.as_u32(), // param_id
                // TODO: Port index
                -1, // port_index
                // TODO: Channel
                -1, // channel
                // TODO: Key
                -1,          // key
                value.value, // value
            );

            buffer.push(event.as_unknown())
        });
        has_param_in_event
    }
}

pub(crate) struct PluginInstanceHost {
    pub id: PluginInstanceID,

    pub audio_ports: Option<PluginAudioPortsExt>,
    pub note_ports: Option<PluginNotePortsExt>,

    pub num_audio_in_channels: usize,
    pub num_audio_out_channels: usize,

    // TODO: main thread shouldn't be optional
    pub main_thread: Option<Box<dyn PluginMainThread>>,
    // TODO: this shouldn't be accessible to the main thread
    pub audio_thread: Option<SharedPluginHostAudioThread>,

    state: Arc<SharedPluginState>,

    save_state: PluginSaveState,
    plugin_version: Option<Shared<String>>,

    param_queues: Option<ParamQueuesMainThread>,
    gesturing_params: FnvHashMap<ParamID, bool>,

    host_request: HostRequestChannelReceiver,
    remove_requested: bool,
    save_state_dirty: bool,
    restarting: bool,
}

impl PluginInstanceHost {
    pub fn new(
        id: PluginInstanceID,
        save_state: PluginSaveState,
        mut main_thread: Option<Box<dyn PluginMainThread>>,
        host_request: HostRequestChannelReceiver,
        plugin_version: Option<Shared<String>>,
    ) -> Self {
        let state = Arc::new(SharedPluginState::new());

        if let Some(preset) = &save_state.preset {
            if let Some(main_thread) = &mut main_thread {
                match main_thread.load_state(preset) {
                    Ok(()) => {
                        log::trace!("Plugin {:?} successfully loaded preset", &id);
                    }
                    Err(e) => {
                        log::error!("Plugin {:?} failed to load preset: {}", &id, e);
                    }
                }
            }
        }

        if main_thread.is_none() {
            state.set_state(PluginState::InactiveWithError);
        }

        let (num_audio_in_channels, num_audio_out_channels) =
            if let Some(backup_audio_ports) = &save_state.backup_audio_ports {
                (backup_audio_ports.total_in_channels(), backup_audio_ports.total_out_channels())
            } else {
                (0, 0)
            };

        Self {
            id,
            main_thread,
            audio_thread: None,
            audio_ports: None,
            note_ports: None,
            num_audio_in_channels,
            num_audio_out_channels,
            state: Arc::new(SharedPluginState::new()),
            save_state,
            plugin_version,
            param_queues: None,
            gesturing_params: FnvHashMap::default(),
            host_request,
            remove_requested: false,
            save_state_dirty: false,
            restarting: false,
        }
    }

    pub fn new_graph_in(
        id: PluginInstanceID,
        host_request: HostRequestChannelReceiver,
        num_audio_out_channels: usize,
    ) -> Self {
        let state = Arc::new(SharedPluginState::new());

        state.set_state(PluginState::Inactive);

        // We don't actually use this save state. This is just here to be
        // consistent with the rest of the plugins.
        let save_state = PluginSaveState {
            key: ScannedPluginKey {
                rdn: "app.meadowlark.dropseed-graph-in".into(),
                format: PluginFormat::Internal,
            },
            is_active: false,
            backup_audio_ports: None,
            backup_note_ports: None,
            preset: None,
        };

        Self {
            id,
            main_thread: None,
            audio_thread: None,
            audio_ports: None,
            note_ports: None,
            num_audio_in_channels: 0,
            num_audio_out_channels,
            state: Arc::new(SharedPluginState::new()),
            save_state,
            plugin_version: None,
            param_queues: None,
            gesturing_params: FnvHashMap::default(),
            host_request,
            remove_requested: false,
            save_state_dirty: false,
            restarting: false,
        }
    }

    pub fn new_graph_out(
        id: PluginInstanceID,
        host_request: HostRequestChannelReceiver,
        num_audio_in_channels: usize,
    ) -> Self {
        let state = Arc::new(SharedPluginState::new());

        state.set_state(PluginState::Inactive);

        // We don't actually use this save state. This is just here to be
        // consistent with the rest of the plugins.
        let save_state = PluginSaveState {
            key: ScannedPluginKey {
                rdn: "app.meadowlark.dropseed-graph-out".into(),
                format: PluginFormat::Internal,
            },
            is_active: false,
            backup_audio_ports: None,
            backup_note_ports: None,
            preset: None,
        };

        Self {
            id,
            main_thread: None,
            audio_thread: None,
            audio_ports: None,
            note_ports: None,
            num_audio_in_channels,
            num_audio_out_channels: 0,
            state: Arc::new(SharedPluginState::new()),
            save_state,
            plugin_version: None,
            param_queues: None,
            gesturing_params: FnvHashMap::default(),
            host_request,
            remove_requested: false,
            save_state_dirty: false,
            restarting: false,
        }
    }

    pub fn collect_save_state(&mut self) -> PluginSaveState {
        if self.save_state_dirty {
            if let Some(main_thread) = &mut self.main_thread {
                let preset = match main_thread.collect_save_state() {
                    Ok(preset) => preset.map(|bytes| PluginPreset {
                        version: self.plugin_version.as_ref().map(|v| String::clone(&*v)),
                        bytes,
                    }),
                    Err(e) => {
                        log::error!(
                            "Failed to collect save state from plugin {:?}: {}",
                            &self.id,
                            e
                        );

                        None
                    }
                };

                self.save_state.preset = preset;
            }
        }

        self.save_state.clone()
    }

    pub fn can_activate(&self) -> Result<(), ActivatePluginError> {
        if self.main_thread.is_none() {
            return Err(ActivatePluginError::NotLoaded);
        }
        // TODO: without this check it seems something is attempting to activate the plugin twice
        if self.state.get_state() == PluginState::Active {
            return Err(ActivatePluginError::AlreadyActive);
        }
        Ok(())
    }

    pub fn activate(
        &mut self,
        sample_rate: SampleRate,
        min_frames: u32,
        max_frames: u32,
        coll_handle: &basedrop::Handle,
    ) -> Result<(PluginHandle, FnvHashMap<ParamID, f64>), ActivatePluginError> {
        self.can_activate()?;

        let plugin_main_thread = self.main_thread.as_mut().unwrap();

        let audio_ports = match plugin_main_thread.audio_ports_ext() {
            Ok(audio_ports) => {
                self.num_audio_in_channels = audio_ports.total_in_channels();
                self.num_audio_out_channels = audio_ports.total_out_channels();

                self.save_state.backup_audio_ports = Some(audio_ports.clone());

                audio_ports
            }
            Err(e) => {
                self.state.set_state(PluginState::InactiveWithError);
                self.audio_ports = None;

                return Err(ActivatePluginError::PluginFailedToGetAudioPortsExt(e));
            }
        };

        let note_ports = match plugin_main_thread.note_ports_ext() {
            Ok(note_ports) => {
                self.save_state.backup_note_ports = Some(note_ports.clone());

                note_ports
            }
            Err(e) => {
                self.state.set_state(PluginState::InactiveWithError);
                self.note_ports = None;

                return Err(ActivatePluginError::PluginFailedToGetNotePortsExt(e));
            }
        };

        self.audio_ports = Some(audio_ports.clone());
        self.note_ports = Some(note_ports.clone());

        let num_params = plugin_main_thread.num_params() as usize;
        let mut params: FnvHashMap<ParamID, ParamInfo> = FnvHashMap::default();
        let mut param_values: FnvHashMap<ParamID, f64> = FnvHashMap::default();

        for i in 0..num_params {
            match plugin_main_thread.param_info(i) {
                Ok(info) => match plugin_main_thread.param_value(info.stable_id) {
                    Ok(value) => {
                        let id = info.stable_id;

                        let _ = params.insert(id, info);
                        let _ = param_values.insert(id, value);
                    }
                    Err(_) => {
                        self.state.set_state(PluginState::InactiveWithError);

                        return Err(ActivatePluginError::PluginFailedToGetParamValue(
                            info.stable_id,
                        ));
                    }
                },
                Err(_) => {
                    self.state.set_state(PluginState::InactiveWithError);

                    return Err(ActivatePluginError::PluginFailedToGetParamInfo(i));
                }
            }
        }

        match plugin_main_thread.activate(sample_rate, min_frames, max_frames, coll_handle) {
            Ok(info) => {
                self.state.set_state(PluginState::Active);

                let mut params_ext = PluginParamsExt {
                    params,
                    ui_to_audio_param_value_tx: None,
                    ui_to_audio_param_mod_tx: None,
                };

                let (param_queues_main_thread, param_queues_audio_thread) = if num_params > 0 {
                    let (ui_to_audio_param_value_tx, ui_to_audio_param_value_rx) =
                        ReducingFnvQueue::new(num_params, coll_handle);
                    let (ui_to_audio_param_mod_tx, ui_to_audio_param_mod_rx) =
                        ReducingFnvQueue::new(num_params, coll_handle);
                    let (audio_to_main_param_value_tx, audio_to_main_param_value_rx) =
                        ReducingFnvQueue::new(num_params, coll_handle);

                    params_ext.ui_to_audio_param_value_tx = Some(ui_to_audio_param_value_tx);
                    params_ext.ui_to_audio_param_mod_tx = Some(ui_to_audio_param_mod_tx);

                    (
                        Some(ParamQueuesMainThread { audio_to_main_param_value_rx }),
                        Some(ParamQueuesAudioThread {
                            audio_to_main_param_value_tx,
                            ui_to_audio_param_value_rx,
                            ui_to_audio_param_mod_rx,
                        }),
                    )
                } else {
                    (None, None)
                };

                self.param_queues = param_queues_main_thread;

                let has_automation_out_port = plugin_main_thread.has_automation_out_port();

                self.audio_thread = Some(SharedPluginHostAudioThread::new(
                    PluginInstanceHostAudioThread {
                        id: self.id.clone(),
                        plugin: info.audio_thread,
                        state: Arc::clone(&self.state),
                        param_queues: param_queues_audio_thread,
                        in_events: EventBuffer::with_capacity(num_params * 3),
                        out_events: EventBuffer::with_capacity(num_params * 3),
                        event_output_sanitizer: PluginEventOutputSanitizer::new(num_params),
                        processing_state: ProcessingState::WaitingForStart,
                    },
                    coll_handle,
                ));

                Ok((
                    PluginHandle {
                        audio_ports,
                        internal: info.internal_handle,
                        note_ports,
                        params: params_ext,
                        has_automation_out_port,
                    },
                    param_values,
                ))
            }
            Err(e) => {
                self.state.set_state(PluginState::InactiveWithError);

                Err(ActivatePluginError::PluginSpecific(e))
            }
        }
    }

    pub fn schedule_deactivate(&mut self) {
        if self.state.get_state() != PluginState::Active {
            return;
        }

        // Allow the plugin's audio thread to be dropped when the new
        // schedule is sent.
        self.audio_thread = None;

        // Wait for the audio thread part to go to sleep before
        // deactivating.
        self.state.set_state(PluginState::WaitingToDrop);
    }

    pub fn schedule_remove(&mut self) {
        self.remove_requested = true;

        self.schedule_deactivate();
    }

    pub fn audio_ports_ext(&self) -> Option<&PluginAudioPortsExt> {
        if self.audio_ports.is_some() {
            self.audio_ports.as_ref()
        } else {
            self.save_state.backup_audio_ports.as_ref()
        }
    }

    pub fn note_ports_ext(&self) -> Option<&PluginNotePortsExt> {
        if self.note_ports.is_some() {
            self.note_ports.as_ref()
        } else {
            self.save_state.backup_note_ports.as_ref()
        }
    }

    /// Whether or not this plugin has an automation out port (seperate from audio
    /// and note out ports).
    ///
    /// Only return `true` for internal plugins which output parameter automation
    /// events for other plugins.
    pub fn has_automation_out_port(&self) -> bool {
        if let Some(main_thread) = &self.main_thread {
            main_thread.has_automation_out_port()
        } else {
            false
        }
    }

    pub fn on_idle(
        &mut self,
        sample_rate: SampleRate,
        min_frames: u32,
        max_frames: u32,
        coll_handle: &basedrop::Handle,
        event_tx: &mut Option<&mut Sender<DSEngineEvent>>,
    ) -> (OnIdleResult, SmallVec<[ParamModifiedInfo; 4]>) {
        let mut modified_params: SmallVec<[ParamModifiedInfo; 4]> = SmallVec::new();

        if self.main_thread.is_none() {
            if self.remove_requested {
                return (OnIdleResult::PluginReadyToRemove, modified_params);
            } else {
                return (OnIdleResult::Ok, modified_params);
            }
        }

        let plugin_main_thread = self.main_thread.as_mut().unwrap();

        let request_flags = self.host_request.fetch_requests();
        let mut state = self.state.get_state();

        if request_flags.contains(HostRequestFlags::CALLBACK) {
            plugin_main_thread.on_main_thread();
        }

        if request_flags.contains(HostRequestFlags::RESTART) && !self.remove_requested {
            self.restarting = true;
            if state != PluginState::DroppedAndReadyToDeactivate {
                self.state.set_state(PluginState::WaitingToDrop);
                state = PluginState::WaitingToDrop;
            }
        }

        if request_flags.intersects(HostRequestFlags::GUI_CLOSED | HostRequestFlags::GUI_DESTROYED)
        {
            plugin_main_thread
                .on_gui_closed(request_flags.contains(HostRequestFlags::GUI_DESTROYED));

            if let Some(event_tx) = event_tx.as_mut() {
                event_tx
                    .send(DSEngineEvent::Plugin(PluginEvent::GuiClosed {
                        plugin_id: self.id.clone(),
                    }))
                    .unwrap()
            }
        }

        if let Some(params_queue) = &mut self.param_queues {
            params_queue.audio_to_main_param_value_rx.consume(|param_id, new_value| {
                let is_gesturing = if let Some(gesture) = new_value.gesture {
                    let _ = self.gesturing_params.insert(*param_id, gesture.is_begin);
                    gesture.is_begin
                } else {
                    *self.gesturing_params.get(param_id).unwrap_or(&false)
                };

                modified_params.push(ParamModifiedInfo {
                    param_id: *param_id,
                    new_value: new_value.value,
                    is_gesturing,
                })
            });
        }

        if state == PluginState::DroppedAndReadyToDeactivate {
            // Safe to deactivate now.
            plugin_main_thread.deactivate();

            self.state.set_state(PluginState::Inactive);

            self.param_queues = None;

            if !self.remove_requested {
                let mut res = OnIdleResult::PluginDeactivated;

                if self.restarting || request_flags.contains(HostRequestFlags::PROCESS) {
                    match self.activate(sample_rate, min_frames, max_frames, coll_handle) {
                        Ok((ui_handle, param_values)) => {
                            res = OnIdleResult::PluginActivated(ui_handle, param_values)
                        }
                        Err(e) => res = OnIdleResult::PluginFailedToActivate(e),
                    }
                }

                return (res, modified_params);
            } else {
                return (OnIdleResult::PluginReadyToRemove, modified_params);
            }
        } else if request_flags.contains(HostRequestFlags::PROCESS)
            && !self.remove_requested
            && !self.restarting
        {
            if state == PluginState::Active {
                self.state.start_processing.store(true, Ordering::Relaxed);
            } else if state == PluginState::Inactive || state == PluginState::InactiveWithError {
                let res = match self.activate(sample_rate, min_frames, max_frames, coll_handle) {
                    Ok((ui_handle, param_values)) => {
                        OnIdleResult::PluginActivated(ui_handle, param_values)
                    }
                    Err(e) => OnIdleResult::PluginFailedToActivate(e),
                };

                return (res, modified_params);
            }
        }

        (OnIdleResult::Ok, modified_params)
    }

    pub fn update_tempo_map(&mut self, new_tempo_map: &Shared<TempoMap>) {
        if let Some(main_thread) = &mut self.main_thread {
            main_thread.update_tempo_map(new_tempo_map);
        }
    }
}

pub(crate) enum OnIdleResult {
    Ok,
    PluginDeactivated,
    PluginActivated(PluginHandle, FnvHashMap<ParamID, f64>),
    PluginReadyToRemove,
    PluginFailedToActivate(ActivatePluginError),
}

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
            self.in_events.clear();
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
            self.in_events.clear();
            return;
        }

        self.out_events.clear();

        let mut has_param_in_event = self
            .param_queues
            .as_mut()
            .map(|q| q.consume_into_event_buffer(&mut self.in_events))
            .unwrap_or(false);

        let (has_note_in_event, wrote_param_in_event) =
            event_buffers.write_input_events(&mut self.in_events);

        has_param_in_event = has_param_in_event || wrote_param_in_event;

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

                self.in_events.clear();
                return;
            }

            self.state.set_state(PluginState::Active);
        }

        let new_status =
            self.plugin.process(proc_info, buffers, &self.in_events, &mut self.out_events);

        self.in_events.clear();

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

        self.out_events.clear();

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum PluginState {
    // TODO: this state shouldn't be able to exist for the Audio thread
    /// The plugin is inactive, only the main thread uses it.
    Inactive = 0,

    /// Activation failed.
    InactiveWithError = 1,

    /// The plugin is active. It may or may not be processing right now.
    Active = 2,

    /// The main thread is waiting for the audio thread to drop the plugin's audio processor.
    WaitingToDrop = 3,

    /// The plugin is not used anymore by the audio engine and can be deactivated on the main.
    /// thread
    DroppedAndReadyToDeactivate = 4,
}

impl From<u32> for PluginState {
    fn from(s: u32) -> Self {
        match s {
            0 => PluginState::Inactive,
            1 => PluginState::InactiveWithError,
            2 => PluginState::Active,
            3 => PluginState::WaitingToDrop,
            4 => PluginState::DroppedAndReadyToDeactivate,
            _ => PluginState::InactiveWithError,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SharedPluginState {
    state: AtomicU32,
    start_processing: AtomicBool,
}

impl SharedPluginState {
    pub fn new() -> Self {
        Self { state: AtomicU32::new(0), start_processing: AtomicBool::new(false) }
    }

    #[inline]
    pub fn get_state(&self) -> PluginState {
        let s = self.state.load(Ordering::SeqCst);

        s.into()
    }

    #[inline]
    pub fn set_state(&self, state: PluginState) {
        self.state.store(state as u32, Ordering::SeqCst);
    }
}

#[derive(Debug)]
pub enum ActivatePluginError {
    NotLoaded,
    AlreadyActive,
    RestartScheduled,
    PluginFailedToGetAudioPortsExt(String),
    PluginFailedToGetNotePortsExt(String),
    PluginFailedToGetParamInfo(usize),
    PluginFailedToGetParamValue(ParamID),
    PluginSpecific(String),
}

impl Error for ActivatePluginError {}

impl std::fmt::Display for ActivatePluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActivatePluginError::NotLoaded => write!(f, "plugin failed to load from disk"),
            ActivatePluginError::AlreadyActive => write!(f, "plugin is already active"),
            ActivatePluginError::RestartScheduled => {
                write!(f, "a restart is scheduled for this plugin")
            }
            ActivatePluginError::PluginFailedToGetAudioPortsExt(e) => {
                write!(f, "plugin returned error while getting audio ports extension: {:?}", e)
            }
            ActivatePluginError::PluginFailedToGetNotePortsExt(e) => {
                write!(f, "plugin returned error while getting note ports extension: {:?}", e)
            }
            ActivatePluginError::PluginFailedToGetParamInfo(index) => {
                write!(f, "plugin returned error while getting parameter info at index: {}", index)
            }
            ActivatePluginError::PluginFailedToGetParamValue(param_id) => {
                write!(
                    f,
                    "plugin returned error while getting parameter value with ID: {:?}",
                    param_id
                )
            }
            ActivatePluginError::PluginSpecific(e) => {
                write!(f, "plugin returned error while activating: {:?}", e)
            }
        }
    }
}

impl From<String> for ActivatePluginError {
    fn from(e: String) -> Self {
        ActivatePluginError::PluginSpecific(e)
    }
}
