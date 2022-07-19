use atomic_refcell::AtomicRefCell;
use audio_graph::{Graph, NodeRef};
use basedrop::Shared;
use crossbeam_channel::Sender;
use fnv::FnvHashMap;
use fnv::FnvHashSet;
use meadowlark_core_types::time::SampleRate;
use smallvec::SmallVec;
use std::error::Error;

pub(crate) mod plugin_host;
pub(crate) mod schedule;
pub(crate) mod shared_pool;

pub(crate) mod buffers;

mod compiler;
mod save_state;
mod verifier;

use dropseed_core::plugin::ext::audio_ports::{MainPortsLayout, PluginAudioPortsExt};
use dropseed_core::plugin::ext::note_ports::PluginNotePortsExt;
use dropseed_core::plugin::ext::params::ParamID;
use dropseed_core::plugin::{
    HostRequestChannelReceiver, PluginInstanceID, PluginInstanceType, PluginSaveState,
};
use dropseed_core::transport::TempoMap;

use plugin_host::OnIdleResult;
use schedule::transport_task::{TransportHandle, TransportTask};
use schedule::{Schedule, SharedSchedule};
use shared_pool::{PluginInstanceHostEntry, SharedPluginPool};
use verifier::Verifier;

use crate::engine::events::from_engine::{DSEngineEvent, PluginEvent};
use crate::engine::main_thread::{EdgeReq, EdgeReqPortID, PluginIDReq};
use crate::engine::plugin_scanner::{NewPluginInstanceError, PluginScanner};
use crate::graph::plugin_host::PluginInstanceHost;
use crate::utils::thread_id::SharedThreadIDs;

use crate::graph::buffers::pool::SharedBufferPool;
pub use compiler::GraphCompilerError;
pub use plugin_host::{
    ActivatePluginError, ParamGestureInfo, ParamModifiedInfo, PluginHandle, PluginParamsExt,
};
pub use save_state::{AudioGraphSaveState, EdgeSaveState};
pub use verifier::VerifyScheduleError;

/// A default port type for general purpose applications
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortType {
    /// Audio ports
    Audio,
    ParamAutomation,
    Note,
}

impl Default for PortType {
    fn default() -> Self {
        PortType::Audio
    }
}

impl audio_graph::PortType for PortType {
    const NUM_TYPES: usize = 3;
    fn id(&self) -> usize {
        *self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortChannelID {
    pub(crate) port_type: PortType,
    pub(crate) port_stable_id: u32,
    pub(crate) is_input: bool,
    pub(crate) port_channel: u16,
}

pub(crate) struct AudioGraph {
    // TODO: make a proper accessor
    pub(crate) shared_plugin_pool: SharedPluginPool,
    shared_buffer_pool: SharedBufferPool,
    verifier: Verifier,

    abstract_graph: Graph<PluginInstanceID, PortChannelID, PortType>,
    coll_handle: basedrop::Handle,

    shared_schedule: SharedSchedule,

    graph_in_node_id: PluginInstanceID,
    graph_out_node_id: PluginInstanceID,

    graph_in_channels: u16,
    graph_out_channels: u16,

    graph_in_rdn: Shared<String>,
    graph_out_rdn: Shared<String>,
    temp_rdn: Shared<String>,

    shared_transport_task: Shared<AtomicRefCell<TransportTask>>,

    sample_rate: SampleRate,
    min_frames: u32,
    max_frames: u32,
}

impl AudioGraph {
    pub fn new(
        coll_handle: basedrop::Handle,
        graph_in_channels: u16,
        graph_out_channels: u16,
        sample_rate: SampleRate,
        min_frames: u32,
        max_frames: u32,
        note_buffer_size: usize,
        event_buffer_size: usize,
        thread_ids: SharedThreadIDs,
    ) -> (Self, SharedSchedule, TransportHandle) {
        //assert!(graph_in_channels > 0);
        assert!(graph_out_channels > 0);

        let abstract_graph = Graph::default();

        let shared_plugin_pool = SharedPluginPool::new();
        let shared_buffer_pool = SharedBufferPool::new(
            max_frames,
            note_buffer_size,
            event_buffer_size,
            coll_handle.clone(),
        );

        let (transport_task, transport_handle) =
            TransportTask::new(None, sample_rate, coll_handle.clone());

        let shared_transport_task = Shared::new(&coll_handle, AtomicRefCell::new(transport_task));

        let (shared_schedule, shared_schedule_clone) = SharedSchedule::new(
            Schedule::new(max_frames as usize, Shared::clone(&shared_transport_task)),
            thread_ids,
            &coll_handle,
        );

        let graph_in_rdn = Shared::new(&coll_handle, String::from("org.rustydaw.graph_in_node"));
        let graph_out_rdn = Shared::new(&coll_handle, String::from("org.rustydaw.graph_out_node"));
        let temp_rdn = Shared::new(&coll_handle, String::from("org.rustydaw.temporary_plugin_rdn"));

        // These will get overwritten in the `reset()` method.
        let graph_in_node_id = PluginInstanceID::_new(
            0,
            0,
            PluginInstanceType::GraphInput,
            Shared::clone(&graph_in_rdn),
        );
        let graph_out_node_id = PluginInstanceID::_new(
            1,
            1,
            PluginInstanceType::GraphOutput,
            Shared::clone(&graph_out_rdn),
        );

        let mut new_self = Self {
            shared_plugin_pool,
            shared_buffer_pool,
            verifier: Verifier::new(),
            abstract_graph,
            coll_handle,
            shared_schedule,
            graph_in_node_id,
            graph_out_node_id,
            graph_in_channels,
            graph_out_channels,
            graph_in_rdn,
            graph_out_rdn,
            temp_rdn,
            shared_transport_task,
            sample_rate,
            min_frames,
            max_frames,
        };

        new_self.reset();

        (new_self, shared_schedule_clone, transport_handle)
    }

    pub fn graph_in_node_id(&self) -> &PluginInstanceID {
        &self.graph_in_node_id
    }

    pub fn graph_out_node_id(&self) -> &PluginInstanceID {
        &self.graph_out_node_id
    }

    pub fn add_new_plugin_instance(
        &mut self,
        save_state: PluginSaveState,
        plugin_scanner: &mut PluginScanner,
        fallback_to_other_formats: bool,
    ) -> NewPluginRes {
        let temp_id = PluginInstanceID::_new(
            0,
            0,
            PluginInstanceType::Unloaded,
            Shared::clone(&self.temp_rdn),
        );

        let node_ref = self.abstract_graph.node(temp_id);

        let activate_plugin = save_state.is_active;

        let res = plugin_scanner.create_plugin(save_state, node_ref, fallback_to_other_formats);

        let plugin_id = res.plugin_host.id.clone();

        self.abstract_graph.set_node_ident(node_ref, plugin_id.clone()).unwrap();

        match res.status {
            Ok(()) => {
                log::debug!("Loaded plugin {:?} successfully", &res.plugin_host.id);
            }
            Err(e) => {
                log::error!(
                    "Failed to load plugin {:?} from save state: {}",
                    &res.plugin_host.id,
                    e
                );
            }
        }

        let supports_gui =
            res.plugin_host.main_thread.as_ref().map(|m| m.supports_gui()).unwrap_or(false);

        let entry = PluginInstanceHostEntry {
            plugin_host: res.plugin_host,
            port_channels_refs: FnvHashMap::default(),
            main_audio_in_port_refs: Vec::new(),
            main_audio_out_port_refs: Vec::new(),
            automation_in_port_ref: None,
            automation_out_port_ref: None,
            main_note_in_port_ref: None,
            main_note_out_port_ref: None,
        };

        if self.shared_plugin_pool.plugins.insert(plugin_id.clone(), entry).is_some() {
            panic!("Something went wrong when allocating a new slot for a plugin");
        }

        let activation_status = if activate_plugin {
            self.activate_plugin_instance(&plugin_id).unwrap()
        } else {
            PluginActivationStatus::Inactive
        };

        NewPluginRes { plugin_id, status: activation_status, supports_gui }
    }

    pub fn activate_plugin_instance(
        &mut self,
        id: &PluginInstanceID,
    ) -> Result<PluginActivationStatus, ()> {
        let entry = if let Some(entry) = self.shared_plugin_pool.plugins.get_mut(id) {
            entry
        } else {
            return Err(());
        };

        if let Err(e) = entry.plugin_host.can_activate() {
            return Ok(PluginActivationStatus::ActivationError(e));
        }

        let activation_status = match entry.plugin_host.activate(
            self.sample_rate,
            self.min_frames as u32,
            self.max_frames as u32,
            &self.coll_handle,
        ) {
            Ok((new_handle, new_param_values)) => {
                PluginActivationStatus::Activated { new_handle, new_param_values }
            }
            Err(e) => PluginActivationStatus::ActivationError(e),
        };

        if let PluginActivationStatus::Activated { new_handle, .. } = &activation_status {
            // Update the number of channels (ports) in our abstract graph.

            update_plugin_ports(
                &mut self.abstract_graph,
                entry,
                new_handle.audio_ports(),
                new_handle.note_ports(),
                entry.plugin_host.has_automation_out_port(),
            );
        }

        Ok(activation_status)
    }

    /// Remove the given plugins from the graph.
    ///
    /// This will also automatically disconnect all edges that were connected to these
    /// plugins.
    ///
    /// Requests to remove the "graph input/output" nodes with the IDs `AudioGraph::graph_in_node_id()`
    /// and `AudioGraph::graph_out_node_id()` will be ignored.
    pub fn remove_plugin_instances(
        &mut self,
        plugin_ids: &[PluginInstanceID],
        affected_plugins: &mut FnvHashSet<PluginInstanceID>,
    ) -> FnvHashSet<PluginInstanceID> {
        let mut removed_plugins: FnvHashSet<PluginInstanceID> = FnvHashSet::default();

        for id in plugin_ids.iter() {
            if id == &self.graph_in_node_id || id == &self.graph_out_node_id {
                if id == &self.graph_in_node_id {
                    log::warn!("Ignored request to remove the graph in node from the audio graph");
                } else {
                    log::warn!("Ignored request to remove the graph out node from the audio graph");
                }
                continue;
            }

            if removed_plugins.insert(id.clone()) {
                if let Ok(edges) = self.get_plugin_edges(&id) {
                    for e in edges.incoming {
                        let _ = affected_plugins.insert(e.src_plugin_id.clone());
                    }
                    for e in edges.outgoing {
                        let _ = affected_plugins.insert(e.dst_plugin_id.clone());
                    }

                    if let Some(plugin) = self.shared_plugin_pool.plugins.get_mut(&id) {
                        plugin.plugin_host.schedule_remove();
                    }

                    if let Err(e) = self.abstract_graph.delete_node(NodeRef::new(id._node_ref())) {
                        log::error!("Abstract node failed to delete node: {}", e);
                    }
                } else {
                    let _ = removed_plugins.remove(&id);
                    log::warn!("Ignored request to remove plugin instance {:?}: Plugin is already removed.", id);
                }
            }
        }

        removed_plugins
    }

    pub fn connect_edge(
        &mut self,
        edge: &EdgeReq,
        src_plugin_id: &PluginInstanceID,
        dst_plugin_id: &PluginInstanceID,
    ) -> Result<(), ConnectEdgeError> {
        let src_entry = if let Some(entry) = self.shared_plugin_pool.plugins.get(src_plugin_id) {
            entry
        } else {
            return Err(ConnectEdgeError {
                error_type: ConnectEdgeErrorType::SrcPluginDoesNotExist,
                edge: edge.clone(),
            });
        };
        let dst_entry = if let Some(entry) = self.shared_plugin_pool.plugins.get(dst_plugin_id) {
            entry
        } else {
            return Err(ConnectEdgeError {
                error_type: ConnectEdgeErrorType::DstPluginDoesNotExist,
                edge: edge.clone(),
            });
        };

        let src_port_ref = match &edge.src_port_id {
            EdgeReqPortID::Main => match edge.edge_type {
                PortType::Audio => {
                    if let Some(port_ref) =
                        src_entry.main_audio_out_port_refs.get(usize::from(edge.src_port_channel))
                    {
                        *port_ref
                    } else {
                        return Err(ConnectEdgeError {
                            error_type: ConnectEdgeErrorType::SrcPortDoesNotExist,
                            edge: edge.clone(),
                        });
                    }
                }
                PortType::ParamAutomation => {
                    if let Some(port_ref) = src_entry.automation_out_port_ref {
                        port_ref
                    } else {
                        return Err(ConnectEdgeError {
                            error_type: ConnectEdgeErrorType::SrcPortDoesNotExist,
                            edge: edge.clone(),
                        });
                    }
                }
                PortType::Note => {
                    if let Some(port_ref) = src_entry.main_note_out_port_ref {
                        port_ref
                    } else {
                        return Err(ConnectEdgeError {
                            error_type: ConnectEdgeErrorType::SrcPortDoesNotExist,
                            edge: edge.clone(),
                        });
                    }
                }
            },
            EdgeReqPortID::StableID(id) => {
                let src_port_id = PortChannelID {
                    port_type: edge.edge_type,
                    port_stable_id: *id,
                    is_input: false,
                    port_channel: edge.src_port_channel,
                };

                if let Some(port_ref) = src_entry.port_channels_refs.get(&src_port_id) {
                    *port_ref
                } else {
                    return Err(ConnectEdgeError {
                        error_type: ConnectEdgeErrorType::SrcPortDoesNotExist,
                        edge: edge.clone(),
                    });
                }
            }
        };

        let dst_port_ref = match &edge.dst_port_id {
            EdgeReqPortID::Main => match edge.edge_type {
                PortType::Audio => {
                    if let Some(port_ref) =
                        dst_entry.main_audio_in_port_refs.get(usize::from(edge.dst_port_channel))
                    {
                        *port_ref
                    } else {
                        return Err(ConnectEdgeError {
                            error_type: ConnectEdgeErrorType::DstPortDoesNotExist,
                            edge: edge.clone(),
                        });
                    }
                }
                PortType::ParamAutomation => {
                    if let Some(port_ref) = dst_entry.automation_in_port_ref {
                        port_ref
                    } else {
                        return Err(ConnectEdgeError {
                            error_type: ConnectEdgeErrorType::DstPortDoesNotExist,
                            edge: edge.clone(),
                        });
                    }
                }
                PortType::Note => {
                    if let Some(port_ref) = dst_entry.main_note_in_port_ref {
                        port_ref
                    } else {
                        return Err(ConnectEdgeError {
                            error_type: ConnectEdgeErrorType::DstPortDoesNotExist,
                            edge: edge.clone(),
                        });
                    }
                }
            },
            EdgeReqPortID::StableID(id) => {
                let src_port_id = PortChannelID {
                    port_type: edge.edge_type,
                    port_stable_id: *id,
                    is_input: true,
                    port_channel: edge.dst_port_channel,
                };

                if let Some(port_ref) = src_entry.port_channels_refs.get(&src_port_id) {
                    *port_ref
                } else {
                    return Err(ConnectEdgeError {
                        error_type: ConnectEdgeErrorType::DstPortDoesNotExist,
                        edge: edge.clone(),
                    });
                }
            }
        };

        if src_plugin_id == dst_plugin_id || src_port_ref == dst_port_ref {
            return Err(ConnectEdgeError {
                error_type: ConnectEdgeErrorType::Cycle,
                edge: edge.clone(),
            });
        }

        match self.abstract_graph.connect(src_port_ref, dst_port_ref) {
            Ok(()) => {
                log::trace!("Successfully connected edge: {:?}", &edge);

                Ok(())
            }
            Err(e) => {
                if let audio_graph::Error::Cycle = e {
                    Err(ConnectEdgeError {
                        error_type: ConnectEdgeErrorType::Cycle,
                        edge: edge.clone(),
                    })
                } else {
                    log::error!("Unexpected edge connect error: {}", e);
                    Err(ConnectEdgeError {
                        error_type: ConnectEdgeErrorType::Unkown,
                        edge: edge.clone(),
                    })
                }
            }
        }
    }

    pub fn disconnect_edge(&mut self, edge: &Edge) -> bool {
        let mut found_ports = None;
        if let Ok(edges) =
            self.abstract_graph.node_edges(NodeRef::new(edge.src_plugin_id._node_ref()))
        {
            // Find the corresponding edge.
            for e in edges.iter() {
                if e.dst_node.as_usize() != edge.dst_plugin_id._node_ref() {
                    continue;
                }
                if e.src_node.as_usize() != edge.src_plugin_id._node_ref() {
                    continue;
                }
                if e.port_type != edge.edge_type {
                    continue;
                }

                let src_port = self.abstract_graph.port_ident(e.src_port).unwrap();
                let dst_port = self.abstract_graph.port_ident(e.dst_port).unwrap();

                if src_port.port_stable_id != edge.src_port_stable_id {
                    continue;
                }
                if dst_port.port_stable_id != edge.dst_port_stable_id {
                    continue;
                }

                if src_port.is_input {
                    continue;
                }
                if !dst_port.is_input {
                    continue;
                }

                if src_port.port_channel != edge.src_port_channel {
                    continue;
                }
                if dst_port.port_channel != edge.dst_port_channel {
                    continue;
                }

                found_ports = Some((e.src_port, e.dst_port));
                break;
            }
        }

        if let Some((src_port, dst_port)) = found_ports {
            if let Err(e) = self.abstract_graph.disconnect(src_port, dst_port) {
                log::error!("Unexpected error while disconnecting edge {:?}: {}", edge, e);
            } else {
                log::trace!("Successfully disconnected edge: {:?}", edge);
            }
            true
        } else {
            log::warn!("Could not disconnect edge: {:?}: Edge was not found in the graph", edge);
            false
        }
    }

    pub fn get_plugin_edges(&self, id: &PluginInstanceID) -> Result<PluginEdges, ()> {
        if let Ok(edges) = self.abstract_graph.node_edges(NodeRef::new(id._node_ref())) {
            let mut incoming: SmallVec<[Edge; 8]> = SmallVec::new();
            let mut outgoing: SmallVec<[Edge; 8]> = SmallVec::new();

            for edge in edges.iter() {
                let src_port = self.abstract_graph.port_ident(edge.src_port).unwrap();
                let dst_port = self.abstract_graph.port_ident(edge.dst_port).unwrap();

                if edge.src_node.as_usize() == id._node_ref() {
                    outgoing.push(Edge {
                        edge_type: edge.port_type,
                        src_plugin_id: id.clone(),
                        dst_plugin_id: self
                            .abstract_graph
                            .node_ident(edge.dst_node)
                            .unwrap()
                            .clone(),
                        src_port_stable_id: src_port.port_stable_id,
                        src_port_channel: src_port.port_channel,
                        dst_port_stable_id: dst_port.port_stable_id,
                        dst_port_channel: dst_port.port_channel,
                    });
                } else {
                    incoming.push(Edge {
                        edge_type: edge.port_type,
                        src_plugin_id: self
                            .abstract_graph
                            .node_ident(edge.src_node)
                            .unwrap()
                            .clone(),
                        dst_plugin_id: id.clone(),
                        src_port_stable_id: src_port.port_stable_id,
                        src_port_channel: src_port.port_channel,
                        dst_port_stable_id: dst_port.port_stable_id,
                        dst_port_channel: dst_port.port_channel,
                    });
                }
            }

            Ok(PluginEdges { incoming, outgoing })
        } else {
            Err(())
        }
    }

    pub fn collect_save_state(&mut self) -> AudioGraphSaveState {
        log::trace!("Collecting audio graph save state...");

        let num_plugins = self.shared_plugin_pool.plugins.len();

        let mut plugin_save_states: Vec<PluginSaveState> = Vec::with_capacity(num_plugins);
        let mut edge_save_states: Vec<EdgeSaveState> = Vec::with_capacity(num_plugins * 3);

        let mut node_ref_to_index: FnvHashMap<usize, usize> = FnvHashMap::default();
        node_ref_to_index.reserve(num_plugins);

        for (index, (plugin_id, plugin_entry)) in
            self.shared_plugin_pool.plugins.iter_mut().enumerate()
        {
            if node_ref_to_index.insert(plugin_id._node_ref(), index).is_some() {
                // In theory this should never happen.
                panic!("More than one plugin with node ref: {:?}", plugin_id._node_ref());
            }

            // These are the only two "plugins" without a save state.
            if plugin_id._node_ref() == self.graph_in_node_id._node_ref()
                || plugin_id._node_ref() == self.graph_out_node_id._node_ref()
            {
                continue;
            }

            plugin_save_states.push(plugin_entry.plugin_host.collect_save_state());
        }

        // Iterate again to get all the edges.
        for plugin_id in self.shared_plugin_pool.plugins.keys() {
            for edge in self.abstract_graph.node_edges(NodeRef::new(plugin_id._node_ref())).unwrap()
            {
                let src_port = self.abstract_graph.port_ident(edge.src_port).unwrap();
                let dst_port = self.abstract_graph.port_ident(edge.dst_port).unwrap();

                edge_save_states.push(EdgeSaveState {
                    edge_type: edge.port_type,
                    src_plugin_i: *node_ref_to_index.get(&edge.src_node.as_usize()).unwrap(),
                    dst_plugin_i: *node_ref_to_index.get(&edge.dst_node.as_usize()).unwrap(),
                    src_port_stable_id: src_port.port_stable_id,
                    src_port_channel: src_port.port_channel,
                    dst_port_stable_id: dst_port.port_stable_id,
                    dst_port_channel: dst_port.port_channel,
                });
            }
        }

        AudioGraphSaveState { plugins: plugin_save_states, edges: edge_save_states }
    }

    pub fn reset(&mut self) {
        // Try to gracefully remove all existing plugins.
        for plugin_entry in self.shared_plugin_pool.plugins.values_mut() {
            // Don't remove the graph in/out "plugins".
            if plugin_entry.plugin_host.id == self.graph_in_node_id
                || plugin_entry.plugin_host.id == self.graph_out_node_id
            {
                continue;
            }

            plugin_entry.plugin_host.schedule_remove();
        }

        // TODO: Check that the audio thread is still alive.
        let audio_thread_is_alive = true;

        if audio_thread_is_alive {
            let start_time = std::time::Instant::now();

            const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

            // Wait for all plugins to be removed.
            while self.shared_plugin_pool.plugins.len() > 2 && start_time.elapsed() < TIMEOUT {
                std::thread::sleep(std::time::Duration::from_millis(10));

                let _ = self.on_idle(None);
            }

            if self.shared_plugin_pool.plugins.len() > 2 {
                log::error!("Timed out while removing all plugins");
            }
        }

        self.shared_schedule.set_new_schedule(
            Schedule::new(self.max_frames as usize, Shared::clone(&self.shared_transport_task)),
            &self.coll_handle,
        );

        self.shared_plugin_pool.plugins.clear();
        self.shared_buffer_pool.remove_excess_buffers(0, 0, 0, 0);

        self.abstract_graph = Graph::default();

        // ---  Add the graph input and graph output nodes to the graph  --------------------------

        let graph_in_node_id = PluginInstanceID::_new(
            0,
            0,
            PluginInstanceType::GraphInput,
            Shared::clone(&self.graph_in_rdn),
        );
        let graph_out_node_id = PluginInstanceID::_new(
            1,
            1,
            PluginInstanceType::GraphOutput,
            Shared::clone(&self.graph_out_rdn),
        );

        let graph_in_node_ref = self.abstract_graph.node(graph_in_node_id.clone());
        let graph_out_node_ref = self.abstract_graph.node(graph_out_node_id.clone());

        let graph_in_node_id = PluginInstanceID::_new(
            graph_in_node_ref.as_usize(),
            0,
            PluginInstanceType::GraphInput,
            Shared::clone(&self.graph_in_rdn),
        );
        let graph_out_node_id = PluginInstanceID::_new(
            graph_out_node_ref.as_usize(),
            1,
            PluginInstanceType::GraphOutput,
            Shared::clone(&self.graph_out_rdn),
        );

        self.abstract_graph.set_node_ident(graph_in_node_ref, graph_in_node_id.clone()).unwrap();
        self.abstract_graph.set_node_ident(graph_out_node_ref, graph_out_node_id.clone()).unwrap();

        let mut graph_in_port_refs: FnvHashMap<PortChannelID, audio_graph::PortRef> =
            FnvHashMap::default();
        let mut graph_in_main_audio_out_port_refs: Vec<audio_graph::PortRef> = Vec::new();
        let mut graph_out_port_refs: FnvHashMap<PortChannelID, audio_graph::PortRef> =
            FnvHashMap::default();
        let mut graph_out_main_audio_in_port_refs: Vec<audio_graph::PortRef> = Vec::new();

        for i in 0..self.graph_in_channels {
            let port_id = PortChannelID {
                port_type: PortType::Audio,
                port_stable_id: 0,
                is_input: false,
                port_channel: i,
            };

            let port_ref =
                self.abstract_graph.port(graph_in_node_ref, PortType::Audio, port_id).unwrap();

            let _ = graph_in_port_refs.insert(port_id, port_ref);

            graph_in_main_audio_out_port_refs.push(port_ref);
        }
        for i in 0..self.graph_out_channels {
            let port_id = PortChannelID {
                port_type: PortType::Audio,
                port_stable_id: 0,
                is_input: true,
                port_channel: i,
            };

            let port_ref =
                self.abstract_graph.port(graph_out_node_ref, PortType::Audio, port_id).unwrap();

            let _ = graph_out_port_refs.insert(port_id, port_ref);

            graph_out_main_audio_in_port_refs.push(port_ref);
        }

        let _ = self.shared_plugin_pool.plugins.insert(
            graph_in_node_id.clone(),
            PluginInstanceHostEntry {
                plugin_host: PluginInstanceHost::new_graph_in(
                    graph_in_node_id.clone(),
                    HostRequestChannelReceiver::new_channel().0, // TODO: not needed (there's no sender)
                    self.graph_in_channels as usize,
                ),
                port_channels_refs: graph_in_port_refs,
                main_audio_in_port_refs: Vec::new(),
                main_audio_out_port_refs: graph_in_main_audio_out_port_refs,
                automation_in_port_ref: None,
                automation_out_port_ref: None,
                main_note_in_port_ref: None,
                main_note_out_port_ref: None,
            },
        );
        let _ = self.shared_plugin_pool.plugins.insert(
            graph_out_node_id,
            PluginInstanceHostEntry {
                plugin_host: PluginInstanceHost::new_graph_out(
                    graph_in_node_id,
                    HostRequestChannelReceiver::new_channel().0, // TODO: not needed (there's no sender)
                    self.graph_out_channels as usize,
                ),
                port_channels_refs: graph_out_port_refs,
                main_audio_in_port_refs: graph_out_main_audio_in_port_refs,
                main_audio_out_port_refs: Vec::new(),
                automation_in_port_ref: None,
                automation_out_port_ref: None,
                main_note_in_port_ref: None,
                main_note_out_port_ref: None,
            },
        );

        // ----------------------------------------------------------------------------------------
    }

    pub fn restore_from_save_state(
        &mut self,
        save_state: &AudioGraphSaveState,
        plugin_scanner: &mut PluginScanner,
        fallback_to_other_formats: bool,
    ) -> (Vec<NewPluginRes>, Vec<(PluginInstanceID, PluginEdges)>) {
        log::info!("Restoring audio graph from save state...");

        self.reset();

        let mut plugin_results: Vec<NewPluginRes> = Vec::with_capacity(save_state.plugins.len());

        for plugin_save_state in save_state.plugins.iter() {
            plugin_results.push(self.add_new_plugin_instance(
                plugin_save_state.clone(),
                plugin_scanner,
                fallback_to_other_formats,
            ));
        }

        for edge_save_state in save_state.edges.iter() {
            if edge_save_state.src_plugin_i >= plugin_results.len() + 2 {
                log::error!(
                    "Could not connect edge from save state {:?}, Source plugin does not exist",
                    edge_save_state
                );
                continue;
            }
            if edge_save_state.dst_plugin_i >= plugin_results.len() + 2 {
                log::error!("Could not connect edge from save state {:?}, Destination plugin does not exist", edge_save_state);
                continue;
            }

            let src_plugin_id = if edge_save_state.src_plugin_i == 0 {
                self.graph_in_node_id.clone()
            } else if edge_save_state.src_plugin_i == 1 {
                self.graph_out_node_id.clone()
            } else {
                plugin_results[edge_save_state.src_plugin_i - 2].plugin_id.clone()
            };

            let dst_plugin_id = if edge_save_state.dst_plugin_i == 0 {
                self.graph_in_node_id.clone()
            } else if edge_save_state.dst_plugin_i == 1 {
                self.graph_out_node_id.clone()
            } else {
                plugin_results[edge_save_state.dst_plugin_i - 2].plugin_id.clone()
            };

            let edge = EdgeReq {
                edge_type: edge_save_state.edge_type,
                src_plugin_id: PluginIDReq::Added(0),
                dst_plugin_id: PluginIDReq::Added(0),
                src_port_id: EdgeReqPortID::StableID(edge_save_state.src_port_stable_id),
                src_port_channel: edge_save_state.src_port_channel,
                dst_port_id: EdgeReqPortID::StableID(edge_save_state.dst_port_stable_id),
                dst_port_channel: edge_save_state.dst_port_channel,
                log_error_on_fail: true,
            };

            if let Err(e) = self.connect_edge(&edge, &src_plugin_id, &dst_plugin_id) {
                log::error!("Could not connect edge from save state {:?}, {}", edge_save_state, e);
            }
        }

        let plugins_new_edges: Vec<(PluginInstanceID, PluginEdges)> = plugin_results
            .iter()
            .map(|plugin_res| {
                let id = plugin_res.plugin_id.clone();
                let edges = self.get_plugin_edges(&id).unwrap();
                (id, edges)
            })
            .collect();

        (plugin_results, plugins_new_edges)
    }

    /// Compile the audio graph into a schedule that is sent to the audio thread.
    ///
    /// If an error is returned then the graph **MUST** be restored with the previous
    /// working save state.
    pub fn compile(&mut self) -> Result<(), GraphCompilerError> {
        match compiler::compile_graph(
            &mut self.shared_plugin_pool,
            &mut self.shared_buffer_pool,
            &mut self.abstract_graph,
            &self.shared_transport_task,
            &self.graph_in_node_id,
            &self.graph_out_node_id,
            &mut self.verifier,
            &self.coll_handle,
        ) {
            Ok(schedule) => {
                log::debug!("Successfully compiled new schedule:\n{:?}", &schedule);

                self.shared_schedule.set_new_schedule(schedule, &self.coll_handle);
                Ok(())
            }
            Err(e) => {
                // Replace the current schedule with an emtpy one now that the graph
                // is in an invalid state.
                self.shared_schedule.set_new_schedule(
                    Schedule::new(
                        self.max_frames as usize,
                        Shared::clone(&self.shared_transport_task),
                    ),
                    &self.coll_handle,
                );
                Err(e)
            }
        }
    }

    pub fn on_idle(&mut self, mut event_tx: Option<&mut Sender<DSEngineEvent>>) -> bool {
        let mut plugins_to_remove: SmallVec<[PluginInstanceID; 4]> = SmallVec::new();

        let mut recompile_graph = false;

        // TODO: Optimize by using some kind of hashmap queue that only iterates over the
        // plugins that have a non-zero host request flag, instead of iterating over every
        // plugin every time?
        for plugin in self.shared_plugin_pool.plugins.values_mut() {
            let (res, modified_params) = plugin.plugin_host.on_idle(
                self.sample_rate,
                self.min_frames,
                self.max_frames,
                &self.coll_handle,
                &mut event_tx,
            );

            match res {
                OnIdleResult::Ok => {}
                OnIdleResult::PluginDeactivated => {
                    recompile_graph = true;

                    println!("plugin deactivated");

                    if let Some(event_tx) = event_tx.as_ref() {
                        event_tx
                            .send(DSEngineEvent::Plugin(PluginEvent::Deactivated {
                                plugin_id: plugin.plugin_host.id.clone(),
                                status: Ok(()),
                            }))
                            .unwrap();
                    }
                }
                OnIdleResult::PluginActivated(new_handle, new_param_values) => {
                    update_plugin_ports(
                        &mut self.abstract_graph,
                        plugin,
                        new_handle.audio_ports(),
                        new_handle.note_ports(),
                        plugin.plugin_host.has_automation_out_port(),
                    );

                    recompile_graph = true;

                    if let Some(event_tx) = event_tx.as_ref() {
                        event_tx
                            .send(DSEngineEvent::Plugin(PluginEvent::Activated {
                                plugin_id: plugin.plugin_host.id.clone(),
                                new_handle,
                                new_param_values,
                            }))
                            .unwrap();
                    }
                }
                OnIdleResult::PluginReadyToRemove => {
                    plugins_to_remove.push(plugin.plugin_host.id.clone());

                    // The user should have already been alerted of the plugin being removed
                    // in a previous `DSEngineEvent::AudioGraphModified` event.
                }
                OnIdleResult::PluginFailedToActivate(e) => {
                    recompile_graph = true;

                    if let Some(event_tx) = event_tx.as_ref() {
                        event_tx
                            .send(DSEngineEvent::Plugin(PluginEvent::Deactivated {
                                plugin_id: plugin.plugin_host.id.clone(),
                                status: Err(e),
                            }))
                            .unwrap();
                    }
                }
            }

            if !modified_params.is_empty() {
                if let Some(event_tx) = event_tx.as_ref() {
                    event_tx
                        .send(DSEngineEvent::Plugin(PluginEvent::ParamsModified {
                            plugin_id: plugin.plugin_host.id.clone(),
                            modified_params: modified_params.to_owned(),
                        }))
                        .unwrap();
                }
            }
        }

        for plugin in plugins_to_remove.iter() {
            let _ = self.shared_plugin_pool.plugins.remove(plugin);
        }

        recompile_graph
    }

    pub fn update_tempo_map(&mut self, new_tempo_map: Shared<TempoMap>) {
        for plugin in self.shared_plugin_pool.plugins.values_mut() {
            plugin.plugin_host.update_tempo_map(&new_tempo_map);
        }
    }
}

fn update_plugin_ports(
    abstract_graph: &mut Graph<PluginInstanceID, PortChannelID, PortType>,
    entry: &mut PluginInstanceHostEntry,
    audio_ports: &PluginAudioPortsExt,
    note_ports: &PluginNotePortsExt,
    has_automation_out_port: bool,
) {
    let mut prev_port_channel_refs = entry.port_channels_refs.clone();
    entry.port_channels_refs.clear();

    entry.main_audio_in_port_refs.clear();
    entry.main_audio_out_port_refs.clear();
    entry.main_note_in_port_ref = None;
    entry.main_note_out_port_ref = None;

    for (audio_port_i, audio_in_port) in audio_ports.inputs.iter().enumerate() {
        for i in 0..audio_in_port.channels {
            let port_id = PortChannelID {
                port_type: PortType::Audio,
                port_stable_id: audio_in_port.stable_id,
                is_input: true,
                port_channel: i,
            };

            let port_ref = if let Some(port_ref) = prev_port_channel_refs.get(&port_id) {
                let port_ref = *port_ref;
                let _ = prev_port_channel_refs.remove(&port_id);
                port_ref
            } else {
                let port_ref = abstract_graph
                    .port(NodeRef::new(entry.plugin_host.id._node_ref()), PortType::Audio, port_id)
                    .unwrap();

                let _ = entry.port_channels_refs.insert(port_id, port_ref);

                port_ref
            };

            if audio_port_i == 0 {
                match audio_ports.main_ports_layout {
                    MainPortsLayout::InOut | MainPortsLayout::InOnly => {
                        entry.main_audio_in_port_refs.push(port_ref);
                    }
                    _ => {}
                }
            }
        }
    }

    for (audio_port_i, audio_out_port) in audio_ports.outputs.iter().enumerate() {
        for i in 0..audio_out_port.channels {
            let port_id = PortChannelID {
                port_type: PortType::Audio,
                port_stable_id: audio_out_port.stable_id,
                is_input: false,
                port_channel: i,
            };

            let port_ref = if let Some(port_ref) = prev_port_channel_refs.get(&port_id) {
                let port_ref = *port_ref;
                let _ = prev_port_channel_refs.remove(&port_id);
                port_ref
            } else {
                let port_ref = abstract_graph
                    .port(NodeRef::new(entry.plugin_host.id._node_ref()), PortType::Audio, port_id)
                    .unwrap();

                let _ = entry.port_channels_refs.insert(port_id, port_ref);

                port_ref
            };

            if audio_port_i == 0 {
                match audio_ports.main_ports_layout {
                    MainPortsLayout::InOut | MainPortsLayout::OutOnly => {
                        entry.main_audio_out_port_refs.push(port_ref);
                    }
                    _ => {}
                }
            }
        }
    }

    const IN_AUTOMATION_PORT_ID: PortChannelID = PortChannelID {
        port_type: PortType::ParamAutomation,
        port_stable_id: 0,
        is_input: true,
        port_channel: 0,
    };
    const OUT_AUTOMATION_PORT_ID: PortChannelID = PortChannelID {
        port_type: PortType::ParamAutomation,
        port_stable_id: 1,
        is_input: false,
        port_channel: 0,
    };

    // Plugins always have one automation in port.
    if prev_port_channel_refs.get(&IN_AUTOMATION_PORT_ID).is_none() {
        let in_port_ref = abstract_graph
            .port(
                NodeRef::new(entry.plugin_host.id._node_ref()),
                PortType::ParamAutomation,
                IN_AUTOMATION_PORT_ID,
            )
            .unwrap();

        let _ = entry.port_channels_refs.insert(IN_AUTOMATION_PORT_ID, in_port_ref);

        entry.automation_in_port_ref = Some(in_port_ref);
    } else {
        let _ = prev_port_channel_refs.remove(&IN_AUTOMATION_PORT_ID);
    }

    if has_automation_out_port {
        if prev_port_channel_refs.get(&OUT_AUTOMATION_PORT_ID).is_none() {
            let out_port_ref = abstract_graph
                .port(
                    NodeRef::new(entry.plugin_host.id._node_ref()),
                    PortType::ParamAutomation,
                    OUT_AUTOMATION_PORT_ID,
                )
                .unwrap();

            let _ = entry.port_channels_refs.insert(OUT_AUTOMATION_PORT_ID, out_port_ref);

            entry.automation_out_port_ref = Some(out_port_ref);
        } else {
            let _ = prev_port_channel_refs.remove(&OUT_AUTOMATION_PORT_ID);
        }
    } else {
        entry.automation_out_port_ref = None;
    }

    for (i, note_in_port) in note_ports.inputs.iter().enumerate() {
        let port_id = PortChannelID {
            port_type: PortType::Note,
            port_stable_id: note_in_port.stable_id,
            is_input: true,
            port_channel: 0,
        };

        let port_ref = if let Some(port_ref) = prev_port_channel_refs.get(&port_id) {
            let port_ref = *port_ref;
            let _ = prev_port_channel_refs.remove(&port_id);

            port_ref
        } else {
            let port_ref = abstract_graph
                .port(NodeRef::new(entry.plugin_host.id._node_ref()), PortType::Note, port_id)
                .unwrap();

            let _ = entry.port_channels_refs.insert(port_id, port_ref);

            port_ref
        };

        if i == 0 {
            entry.main_note_in_port_ref = Some(port_ref);
        }
    }

    for (i, note_out_port) in note_ports.outputs.iter().enumerate() {
        let port_id = PortChannelID {
            port_type: PortType::Note,
            port_stable_id: note_out_port.stable_id,
            is_input: false,
            port_channel: 0,
        };

        let port_ref = if let Some(port_ref) = prev_port_channel_refs.get(&port_id) {
            let port_ref = *port_ref;
            let _ = prev_port_channel_refs.remove(&port_id);

            port_ref
        } else {
            let port_ref = abstract_graph
                .port(NodeRef::new(entry.plugin_host.id._node_ref()), PortType::Note, port_id)
                .unwrap();

            let _ = entry.port_channels_refs.insert(port_id, port_ref);

            port_ref
        };

        if i == 0 {
            entry.main_note_out_port_ref = Some(port_ref);
        }
    }

    for (_, removed_port) in prev_port_channel_refs.drain() {
        abstract_graph.delete_port(removed_port).unwrap();
    }
}

impl Drop for AudioGraph {
    fn drop(&mut self) {
        self.shared_schedule.set_new_schedule(
            Schedule::new(self.max_frames as usize, Shared::clone(&self.shared_transport_task)),
            &self.coll_handle,
        );
    }
}

#[derive(Debug)]
pub enum PluginActivationStatus {
    /// This means the plugin successfully activated and returned
    /// its new audio/event port configuration and its new
    /// parameter configuration.
    Activated { new_handle: PluginHandle, new_param_values: FnvHashMap<ParamID, f64> },

    /// This means that the plugin loaded but did not activate yet. This
    /// can happen when the user loads a project with a deactivated
    /// plugin.
    Inactive,

    /// There was an error loading the plugin.
    LoadError(NewPluginInstanceError),

    /// There was an error activating the plugin.
    ActivationError(ActivatePluginError),
}

#[derive(Debug)]
pub struct NewPluginRes {
    pub plugin_id: PluginInstanceID,

    pub status: PluginActivationStatus,
    pub supports_gui: bool, // TODO: probably doesn't belong here
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginEdges {
    pub incoming: SmallVec<[Edge; 8]>,
    pub outgoing: SmallVec<[Edge; 8]>,
}

impl PluginEdges {
    pub fn emtpy() -> Self {
        Self { incoming: SmallVec::new(), outgoing: SmallVec::new() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub edge_type: PortType,

    pub src_plugin_id: PluginInstanceID,
    pub dst_plugin_id: PluginInstanceID,

    pub src_port_stable_id: u32,
    pub src_port_channel: u16,

    pub dst_port_stable_id: u32,
    pub dst_port_channel: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectEdgeErrorType {
    SrcPluginDoesNotExist,
    DstPluginDoesNotExist,
    SrcPortDoesNotExist,
    DstPortDoesNotExist,
    Cycle,
    Unkown,
}

#[derive(Debug, Clone)]
pub struct ConnectEdgeError {
    pub error_type: ConnectEdgeErrorType,
    pub edge: EdgeReq,
}

impl Error for ConnectEdgeError {}

impl std::fmt::Display for ConnectEdgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.error_type {
            ConnectEdgeErrorType::SrcPluginDoesNotExist => {
                write!(
                    f,
                    "Could not add edge {:?} to graph: Source plugin does not exist",
                    &self.edge
                )
            }
            ConnectEdgeErrorType::DstPluginDoesNotExist => {
                write!(
                    f,
                    "Could not add edge {:?} to graph: Destination plugin does not exist",
                    &self.edge
                )
            }
            ConnectEdgeErrorType::SrcPortDoesNotExist => {
                write!(
                    f,
                    "Could not add edge {:?} to graph: Source port does not exist",
                    &self.edge
                )
            }
            ConnectEdgeErrorType::DstPortDoesNotExist => {
                write!(
                    f,
                    "Could not add edge {:?} to graph: Destination port does not exist",
                    &self.edge
                )
            }
            ConnectEdgeErrorType::Cycle => {
                write!(f, "Could not add edge {:?} to graph: Cycle detected", &self.edge)
            }
            ConnectEdgeErrorType::Unkown => {
                write!(f, "Could not add edge {:?} to graph: Unkown error", &self.edge)
            }
        }
    }
}
