use dlopen::wrapper::{Container, WrapperApi};
use std::{ffi::{c_void, c_char}, ptr::{null_mut}};
use crate::{data::{Contents, Machine}, status::Status, convert};

#[derive(WrapperApi)]
pub struct SchApi {
    new_scheduler: unsafe extern "C" fn() -> *mut c_void,
    stop: unsafe extern "C" fn(sch: *mut c_void),
    join: unsafe extern "C" fn(sch: *mut c_void),
    job_count: unsafe extern "C" fn(sch: *mut c_void) -> usize,
    load: unsafe extern "C" fn(handle: *mut c_void, plugin: *mut *mut c_void) -> bool,
    delete_plugin: unsafe extern "C" fn(plugin: *mut c_void),
    plugin_contents: unsafe extern "C" fn(plugin: *const c_void, json: *mut *mut c_char, size: *mut usize),
    launch_machine: unsafe extern "C" fn(plugin: *const c_void, user_id: u32,
        scheduler: *mut c_void, model_name: *mut c_char, machine_json: *mut c_char) -> *mut c_void
}

impl SchApi {
    pub fn new(lib: String) -> Result<Container<SchApi>, Status> {
        unsafe { convert!(Container::load(lib)) }
    }

    pub fn get_contents(&self, plugin: &PluginAPI) -> Contents {
        let mut json: *mut c_char = null_mut();
        let mut size: usize = 0;
        unsafe {
            self.plugin_contents(plugin.handle(), &mut json, &mut size as *mut usize);
            let json = String::from_raw_parts(json as *mut u8, size, size);
            Contents::from_json(json.as_str()).expect("Json Error")
        }
    }

    pub fn test_plugin(filename: String) -> Result<Contents, Status> {
        let p: Container<PluginApi> = unsafe { convert!(Container::load(filename))? };
        let mut json: *mut c_char = null_mut();
        let mut size: usize = 0;
        unsafe {
            p.contents_json(&mut json, &mut size as *mut usize);
            let json = String::from_raw_parts(json as *mut u8, size, size);
            convert!(Contents::from_json(json.as_str()))
        }
    }
}

#[derive(WrapperApi)]
pub struct PluginApi {
    contents_json: unsafe extern "C" fn(json: *mut *mut c_char, size: *mut usize),
}

#[repr(C)]
pub struct PluginAPI {
    pub handle: *mut c_void,

    // Only in Rust
    pub cnt: Contents
}

impl Drop for PluginAPI {
    fn drop(&mut self) {
        // unsafe { self.api.delete_plugin(self.handle); }
    }
}

impl PluginAPI {
    pub fn new() -> Self {
        Self { handle: null_mut(), cnt: Contents::new() }
    }

    pub fn handle(&self) -> *mut c_void {
        self.handle
    }

    pub fn machines(&self) -> &Vec<Machine> {
        &self.cnt.machines
    }
}