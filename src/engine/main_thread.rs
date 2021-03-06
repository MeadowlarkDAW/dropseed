use basedrop::{Collector, Shared, SharedCell};
use crossbeam_channel::{Receiver, Sender};
use fnv::FnvHashSet;
use log::warn;
use meadowlark_core_types::time::SampleRate;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::JoinHandle;
use std::time::Duration;
use thread_priority::ThreadPriority;

use dropseed_core::plugin::{HostInfo, PluginFactory, PluginInstanceID, PluginSaveState};
use dropseed_core::plugin_scanner::ScannedPluginKey;
use dropseed_core::transport::TempoMap;

use crate::engine::audio_thread::DSEngineAudioThread;
use crate::engine::events::from_engine::{
    DSEngineEvent, EngineDeactivatedInfo, PluginScannerEvent,
};
use crate::engine::events::to_engine::{DSEngineRequest, PluginRequest};
use crate::engine::plugin_scanner::PluginScanner;
use crate::graph::schedule::transport_task::TransportHandle;
use crate::graph::{AudioGraph, AudioGraphSaveState, Edge, NewPluginRes, PluginEdges, PortType};
use crate::utils::thread_id::SharedThreadIDs;

static ENGINE_THREAD_UPDATE_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) struct DSEngineMainThread {
    audio_graph: Option<AudioGraph>,
    plugin_scanner: PluginScanner,
    event_tx: Sender<DSEngineEvent>,
    handle_to_engine_rx: Receiver<DSEngineRequest>,
    thread_ids: SharedThreadIDs,
    collector: Collector,
    run_process_thread: Option<Arc<AtomicBool>>,
    process_thread_handle: Option<JoinHandle<()>>,
    tempo_map_shared: Option<Shared<SharedCell<(Shared<TempoMap>, u64)>>>,
}

impl DSEngineMainThread {
    pub(crate) fn new(
        host_info: HostInfo,
        mut internal_plugins: Vec<Box<dyn PluginFactory>>,
        handle_to_engine_rx: Receiver<DSEngineRequest>,
        event_tx: Sender<DSEngineEvent>,
    ) -> (Self, Vec<Result<ScannedPluginKey, String>>) {
        // Set up and run garbage collector wich collects and safely drops garbage from
        // the audio thread.
        let collector = Collector::new();

        let host_info = Shared::new(&collector.handle(), host_info);

        let thread_ids = SharedThreadIDs::new(None, None, &collector.handle());

        let mut plugin_scanner =
            PluginScanner::new(collector.handle(), host_info, thread_ids.clone());

        // Scan the user's internal plugins.
        let internal_plugins_res: Vec<Result<ScannedPluginKey, String>> =
            internal_plugins.drain(..).map(|p| plugin_scanner.scan_internal_plugin(p)).collect();

        (
            Self {
                audio_graph: None,
                plugin_scanner,
                //garbage_coll_handle: Some(garbage_coll_handle),
                //garbage_coll_run,
                event_tx,
                handle_to_engine_rx,
                thread_ids,
                collector,
                //coll_handle,
                run_process_thread: None,
                process_thread_handle: None,
                tempo_map_shared: None,
            },
            internal_plugins_res,
        )
    }

    pub fn run(&mut self, run: Arc<AtomicBool>) {
        self.thread_ids
            .set_external_main_thread_id(std::thread::current().id(), &self.collector.handle());

        while run.load(Ordering::Relaxed) {
            while let Ok(msg) = self.handle_to_engine_rx.try_recv() {
                match msg {
                    DSEngineRequest::ModifyGraph(req) => self.modify_graph(req),
                    DSEngineRequest::ActivateEngine(settings) => self.activate_engine(&settings),
                    DSEngineRequest::DeactivateEngine => self.deactivate_engine(),
                    DSEngineRequest::RestoreFromSaveState(save_state) => {
                        self.restore_audio_graph_from_save_state(&save_state)
                    }
                    DSEngineRequest::RequestLatestSaveState => self.request_latest_save_state(),

                    #[cfg(feature = "clap-host")]
                    DSEngineRequest::AddClapScanDirectory(path) => {
                        self.add_clap_scan_directory(path)
                    }

                    #[cfg(feature = "clap-host")]
                    DSEngineRequest::RemoveClapScanDirectory(path) => {
                        self.remove_clap_scan_directory(path)
                    }

                    DSEngineRequest::RescanPluginDirectories => self.rescan_plugin_directories(),

                    DSEngineRequest::UpdateTempoMap(new_tempo_map) => {
                        if let Some(tempo_map_shared) = &self.tempo_map_shared {
                            let tempo_map_version = tempo_map_shared.get().1;

                            let new_tempo_map_shared =
                                Shared::new(&self.collector.handle(), *new_tempo_map);

                            tempo_map_shared.set(Shared::new(
                                &self.collector.handle(),
                                (Shared::clone(&new_tempo_map_shared), tempo_map_version + 1),
                            ));

                            if let Some(audio_graph) = &mut self.audio_graph {
                                audio_graph.update_tempo_map(new_tempo_map_shared);
                            }
                        }
                    }
                    DSEngineRequest::Plugin(instance_id, request) => {
                        let plugin = self
                            .audio_graph
                            .as_mut()
                            .and_then(|a| a.shared_plugin_pool.plugins.get_mut(&instance_id));

                        if let Some(plugin) = plugin {
                            if let Some(main_thread) = plugin.plugin_host.main_thread.as_mut() {
                                // TODO: check this in a separate method
                                match request {
                                    PluginRequest::ShowGui => {
                                        if !main_thread.is_gui_open() {
                                            if let Err(e) = main_thread.open_gui(None) {
                                                warn!("{:?}", e)
                                            }
                                        }
                                    }
                                    PluginRequest::CloseGui => {
                                        if main_thread.is_gui_open() {
                                            main_thread.close_gui()
                                        }
                                    }
                                }
                            }
                        } else {
                            warn!("Received plugin request with invalid ID: {:?}", instance_id)
                        }
                    }
                }
            }

            if let Some(audio_graph) = &mut self.audio_graph {
                let recompile = audio_graph.on_idle(Some(&mut self.event_tx));

                if recompile {
                    self.compile_audio_graph();
                }
            }

            self.collector.collect();

            std::thread::sleep(ENGINE_THREAD_UPDATE_INTERVAL);
        }
    }

    #[cfg(feature = "clap-host")]
    fn add_clap_scan_directory<P: Into<PathBuf>>(&mut self, path: P) {
        let path: PathBuf = path.into();
        if self.plugin_scanner.add_clap_scan_directory(path.clone()) {
            self.event_tx.send(PluginScannerEvent::ClapScanPathAdded(path).into()).unwrap();
        }
    }

    #[cfg(feature = "clap-host")]
    fn remove_clap_scan_directory<P: Into<PathBuf>>(&mut self, path: P) {
        let path: PathBuf = path.into();
        if self.plugin_scanner.remove_clap_scan_directory(path.clone()) {
            self.event_tx.send(PluginScannerEvent::ClapScanPathRemoved(path).into()).unwrap();
        }
    }

    fn rescan_plugin_directories(&mut self) {
        let res = self.plugin_scanner.rescan_plugin_directories();
        self.event_tx.send(PluginScannerEvent::RescanFinished(res).into()).unwrap();
    }

    fn activate_engine(&mut self, settings: &ActivateEngineSettings) {
        if self.audio_graph.is_some() {
            log::warn!("Ignored request to activate RustyDAW engine: Engine is already activated");
            return;
        }

        log::info!("Activating RustyDAW engine...");

        let num_audio_in_channels = settings.num_audio_in_channels;
        let num_audio_out_channels = settings.num_audio_out_channels;
        let min_frames = settings.min_frames;
        let max_frames = settings.max_frames;
        let sample_rate = settings.sample_rate;
        let note_buffer_size = settings.note_buffer_size;
        let event_buffer_size = settings.event_buffer_size;

        let (mut audio_graph, shared_schedule, transport_handle) = AudioGraph::new(
            self.collector.handle(),
            num_audio_in_channels,
            num_audio_out_channels,
            sample_rate,
            min_frames,
            max_frames,
            note_buffer_size,
            event_buffer_size,
            self.thread_ids.clone(),
        );

        let graph_in_node_id = audio_graph.graph_in_node_id().clone();
        let graph_out_node_id = audio_graph.graph_out_node_id().clone();

        // TODO: Remove this once compiler is fixed.
        audio_graph
            .connect_edge(
                &EdgeReq {
                    edge_type: PortType::Audio,
                    src_plugin_id: PluginIDReq::Added(0),
                    dst_plugin_id: PluginIDReq::Added(0),
                    src_port_id: EdgeReqPortID::Main,
                    src_port_channel: 0,
                    dst_port_id: EdgeReqPortID::Main,
                    dst_port_channel: 0,
                    log_error_on_fail: true,
                },
                &graph_in_node_id,
                &graph_out_node_id,
            )
            .unwrap();
        audio_graph
            .connect_edge(
                &EdgeReq {
                    edge_type: PortType::Audio,
                    src_plugin_id: PluginIDReq::Added(0),
                    dst_plugin_id: PluginIDReq::Added(0),
                    src_port_id: EdgeReqPortID::Main,
                    src_port_channel: 1,
                    dst_port_id: EdgeReqPortID::Main,
                    dst_port_channel: 1,
                    log_error_on_fail: true,
                },
                &graph_in_node_id,
                &graph_out_node_id,
            )
            .unwrap();

        self.audio_graph = Some(audio_graph);

        self.compile_audio_graph();

        if let Some(audio_graph) = &self.audio_graph {
            log::info!("Successfully activated RustyDAW engine");

            let (audio_thread, mut process_thread) = DSEngineAudioThread::new(
                num_audio_in_channels as usize,
                num_audio_out_channels as usize,
                &self.collector.handle(),
                shared_schedule,
                sample_rate,
            );

            let run_process_thread = Arc::new(AtomicBool::new(true));

            let run_process_thread_clone = Arc::clone(&run_process_thread);

            if let Some(old_run_process_thread) = self.run_process_thread.take() {
                // Just to be sure.
                old_run_process_thread.store(false, Ordering::Relaxed);
            }
            self.run_process_thread = Some(run_process_thread);

            let process_thread_handle =
                thread_priority::spawn(ThreadPriority::Max, move |priority_res| {
                    if let Err(e) = priority_res {
                        log::error!("Failed to set process thread priority to max: {:?}", e);
                    } else {
                        log::info!("Successfully set process thread priority to max");
                    }

                    process_thread.run(run_process_thread_clone);
                });

            self.process_thread_handle = Some(process_thread_handle);

            self.tempo_map_shared = Some(transport_handle.tempo_map_shared());

            let info = EngineActivatedInfo {
                audio_thread,
                graph_in_node_id: audio_graph.graph_in_node_id().clone(),
                graph_out_node_id: audio_graph.graph_out_node_id().clone(),
                sample_rate,
                min_frames,
                max_frames,
                transport_handle,
                num_audio_in_channels,
                num_audio_out_channels,
            };

            self.event_tx.send(DSEngineEvent::EngineActivated(info)).unwrap();
        } else {
            // If this happens then we did something very wrong.
            panic!("Unexpected error: Empty audio graph failed to compile a schedule.");
        }
    }

    fn modify_graph(&mut self, mut req: ModifyGraphRequest) {
        if let Some(audio_graph) = &mut self.audio_graph {
            let mut affected_plugins: FnvHashSet<PluginInstanceID> = FnvHashSet::default();

            for edge in req.disconnect_edges.iter() {
                if audio_graph.disconnect_edge(edge) {
                    let _ = affected_plugins.insert(edge.src_plugin_id.clone());
                    let _ = affected_plugins.insert(edge.dst_plugin_id.clone());
                }
            }

            let mut removed_plugins = audio_graph
                .remove_plugin_instances(&req.remove_plugin_instances, &mut affected_plugins);

            let new_plugins_res: Vec<NewPluginRes> = req
                .add_plugin_instances
                .drain(..)
                .map(|save_state| {
                    audio_graph.add_new_plugin_instance(save_state, &mut self.plugin_scanner, true)
                })
                .collect();

            let new_plugin_ids: Vec<PluginInstanceID> = new_plugins_res
                .iter()
                .map(|res| {
                    let _ = affected_plugins.insert(res.plugin_id.clone());
                    res.plugin_id.clone()
                })
                .collect();

            for edge in req.connect_new_edges.iter() {
                let src_plugin_id = match &edge.src_plugin_id {
                    PluginIDReq::Added(index) => {
                        if let Some(new_plugin_id) = new_plugin_ids.get(*index) {
                            new_plugin_id
                        } else {
                            log::error!(
                                "Could not connect edge {:?}: Source plugin index out of bounds",
                                edge
                            );
                            continue;
                        }
                    }
                    PluginIDReq::Existing(id) => id,
                };

                let dst_plugin_id = match &edge.dst_plugin_id {
                    PluginIDReq::Added(index) => {
                        if let Some(new_plugin_id) = new_plugin_ids.get(*index) {
                            new_plugin_id
                        } else {
                            log::error!(
                                "Could not connect edge {:?}: Destination plugin index out of bounds",
                                edge
                            );
                            continue;
                        }
                    }
                    PluginIDReq::Existing(id) => id,
                };

                if let Err(e) = audio_graph.connect_edge(edge, src_plugin_id, dst_plugin_id) {
                    if edge.log_error_on_fail {
                        log::error!("Could not connect edge: {}", e);
                    } else {
                        #[cfg(debug_assertions)]
                        log::debug!("Could not connect edge: {}", e);
                    }
                } else {
                    // These will always be true.
                    if let PluginIDReq::Existing(id) = &edge.src_plugin_id {
                        let _ = affected_plugins.insert(id.clone());
                    }
                    if let PluginIDReq::Existing(id) = &edge.dst_plugin_id {
                        let _ = affected_plugins.insert(id.clone());
                    }
                }
            }

            // Don't include the graph in/out "plugins" in the result.
            let _ = affected_plugins.remove(audio_graph.graph_in_node_id());
            let _ = affected_plugins.remove(audio_graph.graph_out_node_id());

            let updated_plugin_edges: Vec<(PluginInstanceID, PluginEdges)> = affected_plugins
                .iter()
                .filter(|plugin_id| !removed_plugins.contains(plugin_id))
                .map(|plugin_id| {
                    (plugin_id.clone(), audio_graph.get_plugin_edges(plugin_id).unwrap())
                })
                .collect();

            let removed_plugins = removed_plugins.drain().collect();

            let res = ModifyGraphRes {
                new_plugins: new_plugins_res,
                removed_plugins,
                updated_plugin_edges,
            };

            // TODO: Compile audio graph in a separate thread?
            self.compile_audio_graph();

            self.event_tx.send(DSEngineEvent::AudioGraphModified(res)).unwrap();
        } else {
            log::warn!("Cannot modify audio graph: Engine is deactivated");
        }
    }

    fn deactivate_engine(&mut self) {
        if self.audio_graph.is_none() {
            log::warn!("Ignored request to deactivate engine: Engine is already deactivated");
            return;
        }

        log::info!("Deactivating RustyDAW engine");

        let save_state = self.audio_graph.as_mut().unwrap().collect_save_state();

        self.audio_graph = None;

        if let Some(run_process_thread) = self.run_process_thread.take() {
            run_process_thread.store(false, Ordering::Relaxed);
        }
        self.process_thread_handle = None;

        self.tempo_map_shared = None;

        self.event_tx
            .send(DSEngineEvent::EngineDeactivated(EngineDeactivatedInfo::DeactivatedGracefully {
                recovered_save_state: save_state,
            }))
            .unwrap();
    }

    fn restore_audio_graph_from_save_state(&mut self, save_state: &AudioGraphSaveState) {
        if self.audio_graph.is_none() {
            log::warn!(
                "Ignored request to restore audio graph from save state: Engine is deactivated"
            );
            return;
        }

        log::info!("Restoring audio graph from save state...");

        log::debug!("Save state: {:?}", save_state);

        self.event_tx.send(DSEngineEvent::AudioGraphCleared).unwrap();

        let (plugins_res, plugins_edges) = self
            .audio_graph
            .as_mut()
            .unwrap()
            .restore_from_save_state(save_state, &mut self.plugin_scanner, true);

        self.compile_audio_graph();

        if self.audio_graph.is_some() {
            log::info!("Restoring audio graph from save state successful");

            let save_state = self.audio_graph.as_mut().unwrap().collect_save_state();

            let res = ModifyGraphRes {
                new_plugins: plugins_res,
                removed_plugins: Vec::new(),
                updated_plugin_edges: plugins_edges,
            };

            self.event_tx.send(DSEngineEvent::AudioGraphModified(res)).unwrap();

            self.event_tx.send(DSEngineEvent::NewSaveState(save_state)).unwrap();
        }
    }

    fn request_latest_save_state(&mut self) {
        if self.audio_graph.is_none() {
            log::warn!(
                "Ignored request for the latest audio graph save state: Engine is deactivated"
            );
            return;
        }

        log::trace!("Got request for latest audio graph save state");

        // TODO: Collect save state in a separate thread?
        let save_state = self.audio_graph.as_mut().unwrap().collect_save_state();

        self.event_tx.send(DSEngineEvent::NewSaveState(save_state)).unwrap();
    }

    fn compile_audio_graph(&mut self) {
        if let Some(mut audio_graph) = self.audio_graph.take() {
            match audio_graph.compile() {
                Ok(_) => {
                    self.audio_graph = Some(audio_graph);
                }
                Err(e) => {
                    log::error!("{}", e);

                    if let Some(run_process_thread) = self.run_process_thread.take() {
                        run_process_thread.store(false, Ordering::Relaxed);
                    }
                    self.process_thread_handle = None;

                    // TODO: Try to recover save state?
                    self.event_tx
                        .send(DSEngineEvent::EngineDeactivated(
                            EngineDeactivatedInfo::EngineCrashed {
                                error_msg: format!("{}", e),
                                recovered_save_state: None,
                            },
                        ))
                        .unwrap();

                    // Audio graph is in an invalid state. Drop it and have the user restore
                    // from the last working save state.
                    let _ = audio_graph;
                }
            }
        }
    }
}

impl Drop for DSEngineMainThread {
    fn drop(&mut self) {
        if let Some(run_process_thread) = self.run_process_thread.take() {
            run_process_thread.store(false, Ordering::Relaxed);

            if let Some(process_thread_handle) = self.process_thread_handle.take() {
                if let Err(e) = process_thread_handle.join() {
                    log::error!("Failed to join process thread handle: {:?}", e);
                }
            }
        }

        // Make sure all of the stuff in the audio thread gets dropped properly.
        let _ = self.audio_graph;

        self.collector.collect();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ActivateEngineSettings {
    pub sample_rate: SampleRate,
    pub min_frames: u32,
    pub max_frames: u32,
    pub num_audio_in_channels: u16,
    pub num_audio_out_channels: u16,
    pub note_buffer_size: usize,
    pub event_buffer_size: usize,
}

impl Default for ActivateEngineSettings {
    fn default() -> Self {
        Self {
            sample_rate: SampleRate::default(),
            min_frames: 1,
            max_frames: 512,
            num_audio_in_channels: 2,
            num_audio_out_channels: 2,
            note_buffer_size: 256,
            event_buffer_size: 256,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginIDReq {
    /// Use an existing plugin in the audio graph.
    Existing(PluginInstanceID),
    /// Use one of the new plugins defined in `ModifyGraphRequest::add_plugin_instances`
    /// (the index into that Vec).
    Added(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum EdgeReqPortID {
    /// Use the main port.
    ///
    /// This can be useful if you don't know the layout of the plugin's ports yet
    /// (because the plugin hasn't been added to the graph yet and activated).
    Main,
    /// Use the port with this specific stable ID.
    StableID(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeReq {
    pub edge_type: PortType,

    pub src_plugin_id: PluginIDReq,
    pub dst_plugin_id: PluginIDReq,

    pub src_port_id: EdgeReqPortID,
    pub src_port_channel: u16,

    pub dst_port_id: EdgeReqPortID,
    pub dst_port_channel: u16,

    /// If true, then the engine should log the error if it failed to connect this edge
    /// for any reason.
    ///
    /// If false, then the engine should not log the error if it failed to connect this
    /// edge for any reason. This can be useful in the common case where when adding a
    /// new plugin to the graph, and you don't know the layout of the plugin's ports yet
    /// (because it hasn't been added to the graph yet and activated), yet you still want
    /// to try and connect any main stereo inputs/outputs to the graph.
    pub log_error_on_fail: bool,
}

#[derive(Debug, Clone)]
pub struct ModifyGraphRequest {
    /// Any new plugin instances to add.
    pub add_plugin_instances: Vec<PluginSaveState>,

    /// Any plugins to remove.
    pub remove_plugin_instances: Vec<PluginInstanceID>,

    /// Any new connections between plugins to add.
    pub connect_new_edges: Vec<EdgeReq>,

    /// Any connections between plugins to remove.
    pub disconnect_edges: Vec<Edge>,
}

#[derive(Debug)]
pub struct ModifyGraphRes {
    /// Any new plugins that were added to the graph.
    pub new_plugins: Vec<NewPluginRes>,

    /// Any plugins that were removed from the graph.
    pub removed_plugins: Vec<PluginInstanceID>,

    ///
    pub updated_plugin_edges: Vec<(PluginInstanceID, PluginEdges)>,
}

pub struct EngineActivatedInfo {
    /// The realtime-safe channel for the audio thread to interface with
    /// the engine.
    ///
    /// Send this to the audio thread to be run.
    ///
    /// When a `DSEngineEvent::EngineDeactivated` event is recieved, send
    /// a signal to the audio thread to drop this.
    pub audio_thread: DSEngineAudioThread,

    /// The ID for the input to the audio graph. Use this to connect any
    /// plugins to system inputs.
    pub graph_in_node_id: PluginInstanceID,

    /// The ID for the output to the audio graph. Use this to connect any
    /// plugins to system outputs.
    pub graph_out_node_id: PluginInstanceID,

    pub transport_handle: TransportHandle,

    pub sample_rate: SampleRate,
    pub min_frames: u32,
    pub max_frames: u32,
    pub num_audio_in_channels: u16,
    pub num_audio_out_channels: u16,
}

impl std::fmt::Debug for EngineActivatedInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("EngineActivatedInfo");

        f.field("graph_in_node_id", &self.graph_in_node_id);
        f.field("graph_out_node_id", &self.graph_out_node_id);
        f.field("sample_rate", &self.sample_rate);
        f.field("min_frames", &self.min_frames);
        f.field("max_frames", &self.max_frames);
        f.field("num_audio_in_channels", &self.num_audio_in_channels);
        f.field("num_audio_out_channels", &self.num_audio_out_channels);

        f.finish()
    }
}
