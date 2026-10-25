use std::path::PathBuf;

#[cfg(windows)]
mod windows {
    use super::PathBuf;
    use std::{
        ffi::{OsStr, OsString, c_void},
        mem::size_of,
        os::windows::ffi::{OsStrExt, OsStringExt},
        ptr::{null, null_mut},
    };

    #[repr(C)]
    struct OpenFileNameW {
        struct_size: u32,
        owner: *mut c_void,
        instance: *mut c_void,
        filter: *const u16,
        custom_filter: *mut u16,
        max_custom_filter: u32,
        filter_index: u32,
        file: *mut u16,
        max_file: u32,
        file_title: *mut u16,
        max_file_title: u32,
        initial_dir: *const u16,
        title: *const u16,
        flags: u32,
        file_offset: u16,
        file_extension: u16,
        default_extension: *const u16,
        custom_data: isize,
        hook: *mut c_void,
        template_name: *const u16,
        reserved: *mut c_void,
        reserved_word: u32,
        flags_ex: u32,
    }

    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(file_name: *mut OpenFileNameW) -> i32;
        fn GetSaveFileNameW(file_name: *mut OpenFileNameW) -> i32;
        fn CommDlgExtendedError() -> u32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetActiveWindow() -> *mut c_void;
    }

    const EXPLORER: u32 = 0x0008_0000;
    const FILE_MUST_EXIST: u32 = 0x0000_1000;
    const PATH_MUST_EXIST: u32 = 0x0000_0800;
    const NO_CHANGE_DIR: u32 = 0x0000_0008;
    const OVERWRITE_PROMPT: u32 = 0x0000_0002;

    pub fn choose(initial: &str, save: bool, genome: bool) -> Result<Option<PathBuf>, String> {
        let filter: Vec<u16> = if genome {
            "Cell Lab genomes\0*.genome\0All files\0*.*\0\0"
        } else {
            "Cell Lab plates\0*.substrate;*.celllab2\0Original Cell Lab\0*.substrate\0Windows Cell Lab\0*.celllab2\0All files\0*.*\0\0"
        }
        .encode_utf16()
        .collect();
        let title: Vec<u16> = match (save, genome) {
            (true, true) => "Save Cell Lab genome\0",
            (false, true) => "Open Cell Lab genome\0",
            (true, false) => "Save Cell Lab plate\0",
            (false, false) => "Open Cell Lab plate\0",
        }
        .encode_utf16()
        .collect();
        let mut file = vec![0u16; 32_768];
        let initial: Vec<u16> = OsStr::new(initial).encode_wide().collect();
        if initial.len() < file.len() {
            file[..initial.len()].copy_from_slice(&initial);
        }
        // OPENFILENAMEW contains only pointers and integers, for which zero is valid.
        let mut request: OpenFileNameW = unsafe { std::mem::zeroed() };
        request.struct_size = size_of::<OpenFileNameW>() as u32;
        request.owner = unsafe { GetActiveWindow() };
        request.instance = null_mut();
        request.filter = filter.as_ptr();
        request.filter_index = 1;
        request.file = file.as_mut_ptr();
        request.max_file = file.len() as u32;
        request.initial_dir = null();
        request.title = title.as_ptr();
        request.default_extension = null();
        request.flags = EXPLORER | PATH_MUST_EXIST | NO_CHANGE_DIR;
        if save {
            request.flags |= OVERWRITE_PROMPT;
        } else {
            request.flags |= FILE_MUST_EXIST;
        }
        let selected = unsafe {
            if save {
                GetSaveFileNameW(&mut request)
            } else {
                GetOpenFileNameW(&mut request)
            }
        };
        if selected == 0 {
            let error = unsafe { CommDlgExtendedError() };
            return if error == 0 {
                Ok(None)
            } else {
                Err(format!("Windows file dialog error 0x{error:04x}"))
            };
        }
        let length = file
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(file.len());
        let mut path = PathBuf::from(OsString::from_wide(&file[..length]));
        if save && path.extension().is_none() {
            path.set_extension(if genome {
                "genome"
            } else if request.filter_index == 2 {
                "substrate"
            } else {
                "celllab2"
            });
        }
        Ok(Some(path))
    }
}

#[cfg(windows)]
pub fn open(initial: &str) -> Result<Option<PathBuf>, String> {
    windows::choose(initial, false, false)
}

#[cfg(windows)]
pub fn save(initial: &str) -> Result<Option<PathBuf>, String> {
    windows::choose(initial, true, false)
}

#[cfg(windows)]
pub fn open_genome(initial: &str) -> Result<Option<PathBuf>, String> {
    windows::choose(initial, false, true)
}

#[cfg(windows)]
pub fn save_genome(initial: &str) -> Result<Option<PathBuf>, String> {
    windows::choose(initial, true, true)
}

#[cfg(not(windows))]
pub fn open(_initial: &str) -> Result<Option<PathBuf>, String> {
    Err("Native file dialogs are only available on Windows".into())
}

#[cfg(not(windows))]
pub fn save(_initial: &str) -> Result<Option<PathBuf>, String> {
    Err("Native file dialogs are only available on Windows".into())
}

#[cfg(not(windows))]
pub fn open_genome(_initial: &str) -> Result<Option<PathBuf>, String> {
    Err("Native file dialogs are only available on Windows".into())
}

#[cfg(not(windows))]
pub fn save_genome(_initial: &str) -> Result<Option<PathBuf>, String> {
    Err("Native file dialogs are only available on Windows".into())
}
