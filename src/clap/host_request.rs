use basedrop::Shared;
use clap_sys::host::clap_host as RawClapHost;
use clap_sys::version::CLAP_VERSION;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::{graph::plugin_pool::PluginInstanceChannel, host_request::HostRequest};

pub(crate) struct ClapHostRequest {
    host_request: HostRequest,
    // We are storing this as a slice so we can get a raw pointer
    // for external plugins.
    raw: Shared<[RawClapHost; 1]>,
    // We are storing this as a slice so we can get a raw pointer
    // for external plugins.
    host_data: Shared<[HostData; 1]>,
}

impl ClapHostRequest {
    pub(crate) fn new(host_request: HostRequest, coll_handle: &basedrop::Handle) -> Self {
        let host_data = Shared::new(
            coll_handle,
            [HostData {
                plug_did_init: Arc::new(AtomicBool::new(false)),
                plugin_channel: Shared::clone(&host_request.plugin_channel),
            }],
        );

        // SAFETY: This is safe because the data lives inside the Host struct,
        // which ensures that the data is alive for as long as there is a
        // reference to it.
        //
        // In addition, this data is wrapped inside basedrop's `Shared` pointer,
        // which ensures that the underlying data doesn't move.
        let raw = Shared::new(
            coll_handle,
            [RawClapHost {
                clap_version: CLAP_VERSION,

                host_data: (*host_data).as_ptr() as *mut c_void,

                name: host_request.info._c_name.as_ptr(),
                vendor: host_request.info._c_vendor.as_ptr(),
                url: host_request.info._c_url.as_ptr(),
                version: host_request.info._c_version.as_ptr(),

                // This is safe because these functions are static.
                get_extension,
                request_restart,
                request_process,
                request_callback,
            }],
        );

        Self { host_request, raw, host_data }
    }

    // SAFETY: This is safe because the data lives inside this struct,
    // which ensures that the data is alive for as long as there is a
    // reference to it.
    //
    // In addition, this data is wrapped inside basedrop's `Shared` pointer,
    // which ensures that the underlying data doesn't move.
    pub(crate) fn get_raw(&self) -> *const RawClapHost {
        (*self.raw).as_ptr()
    }
}

impl Clone for ClapHostRequest {
    fn clone(&self) -> Self {
        Self {
            host_request: self.host_request.clone(),
            raw: Shared::clone(&self.raw),
            host_data: Shared::clone(&self.host_data),
        }
    }
}

struct HostData {
    plug_did_init: Arc<AtomicBool>,
    plugin_channel: Shared<PluginInstanceChannel>,
}

unsafe extern "C" fn get_extension(
    clap_host: *const RawClapHost,
    extension_id: *const i8,
) -> *const c_void {
    if clap_host.is_null() {
        log::warn!(
            "Call to `get_extension(host: *const clap_host, extension_id: *const i8) received a null pointer from plugin`"
        );
        return ptr::null();
    }

    let host = &*(clap_host as *const RawClapHost);

    if host.host_data.is_null() {
        log::warn!(
            "Call to `get_extension(host: *const clap_host, extension_id: *const i8) received a null pointer in host_data from plugin`"
        );
        return ptr::null();
    }

    let host_data = &*(host.host_data as *const HostData);

    if extension_id.is_null() {
        log::warn!(
            "Call to `get_extension(host: *const clap_host, extension_id: *const i8) received a null pointer in extension_id from plugin`"
        );
        return ptr::null();
    }

    if !host_data.plug_did_init.load(Ordering::Relaxed) {
        log::warn!(
            "The plugin can't query for extensions during the create method. Wait for the clap_plugin.init() call."
        );
        return ptr::null();
    }

    // TODO: extensions
    ptr::null()
}

unsafe extern "C" fn request_restart(clap_host: *const RawClapHost) {
    if clap_host.is_null() {
        log::warn!(
            "Call to `request_restart(host: *const clap_host) received a null pointer from plugin`"
        );
        return;
    }

    let host = &*(clap_host as *const RawClapHost);

    if host.host_data.is_null() {
        log::warn!(
            "Call to `request_restart(host: *const clap_host) received a null pointer in host_data from plugin`"
        );
        return;
    }

    let plugin_channel = &*(host.host_data as *const PluginInstanceChannel);

    plugin_channel.restart_requested.store(true, Ordering::Relaxed);
}

unsafe extern "C" fn request_process(clap_host: *const RawClapHost) {
    if clap_host.is_null() {
        log::warn!(
            "Call to `request_process(host: *const clap_host) received a null pointer from plugin`"
        );
        return;
    }

    let host = &*(clap_host as *const RawClapHost);

    if host.host_data.is_null() {
        log::warn!(
            "Call to `request_process(host: *const clap_host) received a null pointer in host_data from plugin`"
        );
        return;
    }

    let plugin_channel = &*(host.host_data as *const PluginInstanceChannel);

    plugin_channel.process_requested.store(true, Ordering::Relaxed);
}

unsafe extern "C" fn request_callback(clap_host: *const RawClapHost) {
    if clap_host.is_null() {
        log::warn!(
            "Call to `request_callback(host: *const clap_host) received a null pointer from plugin`"
        );
        return;
    }

    let host = &*(clap_host as *const RawClapHost);

    if host.host_data.is_null() {
        log::warn!(
            "Call to `request_callback(host: *const clap_host) received a null pointer in host_data from plugin`"
        );
        return;
    }

    let plugin_channel = &*(host.host_data as *const PluginInstanceChannel);

    plugin_channel.callback_requested.store(true, Ordering::Relaxed);
}
