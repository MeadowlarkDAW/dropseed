use dropseed_core::plugin::PluginInstanceID;
use std::path::PathBuf;

use dropseed_core::transport::TempoMap;

use crate::engine::main_thread::{ActivateEngineSettings, ModifyGraphRequest};
use crate::graph::AudioGraphSaveState;

#[derive(Debug, Clone)]
/// A request to the engine.
///
/// Note that the engine may decide to ignore invalid requests.
pub enum DSEngineRequest {
    /// Modify the audio graph.
    ModifyGraph(ModifyGraphRequest),

    /// Activate the engine.
    ActivateEngine(Box<ActivateEngineSettings>),

    /// Deactivate the engine.
    ///
    /// The engine cannot be used until it is reactivated.
    DeactivateEngine,

    /// Restore the engine from a save state.
    RestoreFromSaveState(AudioGraphSaveState),

    /// Request the engine to return the latest save state.
    RequestLatestSaveState,

    #[cfg(feature = "clap-host")]
    /// Add a directory to the list of directories to scan for CLAP plugins.
    AddClapScanDirectory(PathBuf),

    #[cfg(feature = "clap-host")]
    /// Remove a directory from the list of directories to scan for CLAP plugins.
    RemoveClapScanDirectory(PathBuf),

    /// Rescan all plugin directories.
    RescanPluginDirectories,

    UpdateTempoMap(Box<TempoMap>),

    /// A request to a specific Plugin instance
    Plugin(PluginInstanceID, PluginRequest),
}

impl From<ModifyGraphRequest> for DSEngineRequest {
    fn from(m: ModifyGraphRequest) -> Self {
        DSEngineRequest::ModifyGraph(m)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
/// A request to a specific instantiated Plugin
pub enum PluginRequest {
    ShowGui,
    CloseGui,
}
