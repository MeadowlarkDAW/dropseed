use atomic_refcell::AtomicRefCell;
use basedrop::Shared;
use fnv::FnvHashMap;
use std::hash::Hash;

use dropseed_core::plugin::PluginInstanceID;

use super::plugin_host::{PluginInstanceHost, PluginInstanceHostAudioThread};
use super::schedule::delay_comp_node::DelayCompNode;
use super::PortChannelID;

pub(crate) struct SharedPluginHostAudioThread {
    pub plugin: Shared<AtomicRefCell<PluginInstanceHostAudioThread>>,
}

impl SharedPluginHostAudioThread {
    pub fn new(plugin: PluginInstanceHostAudioThread, coll_handle: &basedrop::Handle) -> Self {
        Self { plugin: Shared::new(coll_handle, AtomicRefCell::new(plugin)) }
    }
}

impl SharedPluginHostAudioThread {
    pub fn id(&self) -> PluginInstanceID {
        self.plugin.borrow().id.clone()
    }
}

impl Clone for SharedPluginHostAudioThread {
    fn clone(&self) -> Self {
        Self { plugin: Shared::clone(&self.plugin) }
    }
}

pub(crate) struct PluginInstanceHostEntry {
    pub plugin_host: PluginInstanceHost,
    //pub audio_thread: Option<SharedPluginHostAudioThread>,
    pub port_channels_refs: FnvHashMap<PortChannelID, audio_graph::PortRef>,
    pub main_audio_in_port_refs: Vec<audio_graph::PortRef>,
    pub main_audio_out_port_refs: Vec<audio_graph::PortRef>,
    pub automation_in_port_ref: Option<audio_graph::PortRef>,
    pub automation_out_port_ref: Option<audio_graph::PortRef>,
    pub main_note_in_port_ref: Option<audio_graph::PortRef>,
    pub main_note_out_port_ref: Option<audio_graph::PortRef>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DelayCompKey {
    pub src_node_ref: usize,
    pub port_stable_id: u32,
    pub port_channel_index: u16,
    pub delay: u32,
}

pub(crate) struct SharedDelayCompNode {
    pub node: Shared<AtomicRefCell<DelayCompNode>>,
    pub active: bool,
}

impl SharedDelayCompNode {
    pub fn new(delay: u32, coll_handle: &basedrop::Handle) -> Self {
        Self {
            node: Shared::new(coll_handle, AtomicRefCell::new(DelayCompNode::new(delay))),
            active: true,
        }
    }
}

impl Clone for SharedDelayCompNode {
    fn clone(&self) -> Self {
        Self { node: Shared::clone(&self.node), active: self.active }
    }
}

impl SharedDelayCompNode {
    pub fn delay(&self) -> u32 {
        self.node.borrow().delay()
    }
}

pub(crate) struct SharedPluginPool {
    pub plugins: FnvHashMap<PluginInstanceID, PluginInstanceHostEntry>,
    pub delay_comp_nodes: FnvHashMap<DelayCompKey, SharedDelayCompNode>,
}

impl SharedPluginPool {
    pub fn new() -> Self {
        Self { plugins: FnvHashMap::default(), delay_comp_nodes: FnvHashMap::default() }
    }
}
