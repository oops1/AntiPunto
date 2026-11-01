//! Минимальный FFI к openvino_c.dll, загрузка на лету.

use std::ffi::{CStr, CString, c_char, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null_mut;

use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library, Symbol};

pub const W: usize = 64;

#[repr(C)]
struct Property {
    key: *const c_char,
    value: *const c_void,
}

type Status = i32;
type CoreCreate = unsafe extern "C" fn(*mut *mut c_void) -> Status;
type CompileFromFile =
    unsafe extern "C" fn(*const c_void, *const u16, *const c_char, usize, *const Property, *mut *mut c_void) -> Status;
type CreateRequest = unsafe extern "C" fn(*const c_void, *mut *mut c_void) -> Status;
type GetTensor = unsafe extern "C" fn(*const c_void, usize, *mut *mut c_void) -> Status;
type Infer = unsafe extern "C" fn(*mut c_void) -> Status;
type TensorData = unsafe extern "C" fn(*const c_void, *mut *mut c_void) -> Status;
type TensorSize = unsafe extern "C" fn(*const c_void, *mut usize) -> Status;
type Free = unsafe extern "C" fn(*mut c_void);
type ErrInfo = unsafe extern "C" fn(Status) -> *const c_char;
type LastErr = unsafe extern "C" fn() -> *const c_char;

pub struct Model {
    _lib: Library,
    infer: Symbol<Infer>,
    tensor_free: Symbol<Free>,
    request_free: Symbol<Free>,
    compiled_free: Symbol<Free>,
    core_free: Symbol<Free>,
    core: *mut c_void,
    compiled: *mut c_void,
    request: *mut c_void,
    tensors: Vec<*mut c_void>,
    inputs: Vec<(*mut i32, usize)>,
    out: (*const f32, usize),
    pub device: String,
}

fn check(lib: &Library, st: Status, what: &str) -> Result<(), String> {
    if st == 0 {
        return Ok(());
    }
    let mut msg = format!("{what}: статус {st}");
    unsafe {
        if let Ok(f) = lib.get::<LastErr>(b"ov_get_last_err_msg\0") {
            let p = f();
            if !p.is_null() {
                msg += &format!(" — {}", CStr::from_ptr(p).to_string_lossy());
            }
        } else if let Ok(f) = lib.get::<ErrInfo>(b"ov_get_error_info\0") {
            let p = f(st);
            if !p.is_null() {
                msg += &format!(" — {}", CStr::from_ptr(p).to_string_lossy());
            }
        }
    }
    Err(msg)
}

impl Model {
    /// Модель раскладки: два входа [1, W], выход [1, W].
    pub fn load(ov_dir: &Path, model: &Path, devices: &[&str], cache_dir: &Path) -> Result<Model, String> {
        Model::load_shaped(ov_dir, model, devices, cache_dir, &[W, W], W)
    }

    /// Загрузка модели: устройства по порядку, первое удачное; входы i32, выход f32.
    pub fn load_shaped(
        ov_dir: &Path,
        model: &Path,
        devices: &[&str],
        cache_dir: &Path,
        input_lens: &[usize],
        output_len: usize,
    ) -> Result<Model, String> {
        let lib = unsafe { Library::load_with_flags(ov_dir.join("openvino_c.dll"), LOAD_WITH_ALTERED_SEARCH_PATH) }
            .map_err(|e| format!("openvino_c.dll: {e}"))?;
        unsafe {
            let core_create: Symbol<CoreCreate> = lib.get(b"ov_core_create\0").map_err(|e| e.to_string())?;
            let compile: Symbol<CompileFromFile> =
                lib.get(b"ov_core_compile_model_from_file_unicode_props\0").map_err(|e| e.to_string())?;
            let create_req: Symbol<CreateRequest> =
                lib.get(b"ov_compiled_model_create_infer_request\0").map_err(|e| e.to_string())?;
            let get_in: Symbol<GetTensor> =
                lib.get(b"ov_infer_request_get_input_tensor_by_index\0").map_err(|e| e.to_string())?;
            let get_out: Symbol<GetTensor> =
                lib.get(b"ov_infer_request_get_output_tensor_by_index\0").map_err(|e| e.to_string())?;
            let data: Symbol<TensorData> = lib.get(b"ov_tensor_data\0").map_err(|e| e.to_string())?;
            let size: Symbol<TensorSize> = lib.get(b"ov_tensor_get_byte_size\0").map_err(|e| e.to_string())?;

            let mut core = null_mut();
            check(&lib, core_create(&mut core), "ov_core_create")?;

            let wpath: Vec<u16> = model.as_os_str().encode_wide().chain([0]).collect();
            let cache = CString::new(cache_dir.to_string_lossy().as_bytes()).unwrap();
            let mut compiled = null_mut();
            let mut device = String::new();
            let mut errors = Vec::new();
            for dev in devices {
                let d = CString::new(*dev).unwrap();
                let mut props = vec![
                    Property { key: c"CACHE_DIR".as_ptr(), value: cache.as_ptr().cast() },
                    Property { key: c"PERFORMANCE_HINT".as_ptr(), value: c"LATENCY".as_ptr().cast() },
                ];
                // без bf16/f16 на CPU: одинаковые решения на любой машине
                if *dev == "CPU" {
                    props.push(Property { key: c"INFERENCE_PRECISION_HINT".as_ptr(), value: c"f32".as_ptr().cast() });
                }
                match check(
                    &lib,
                    compile(core, wpath.as_ptr(), d.as_ptr(), props.len(), props.as_ptr(), &mut compiled),
                    dev,
                ) {
                    Ok(()) => {
                        device = dev.to_string();
                        break;
                    }
                    Err(e) => errors.push(e),
                }
            }
            if compiled.is_null() {
                return Err(errors.join("; "));
            }

            let mut request = null_mut();
            check(&lib, create_req(compiled, &mut request), "create_infer_request")?;
            let mut tensors = Vec::new();
            let mut ptrs = Vec::new();
            let lens: Vec<usize> = input_lens.iter().copied().chain([output_len]).collect();
            for (i, &len) in lens.iter().enumerate() {
                let mut t = null_mut();
                if i < input_lens.len() {
                    check(&lib, get_in(request, i, &mut t), "input")?;
                } else {
                    check(&lib, get_out(request, 0, &mut t), "output")?;
                }
                tensors.push(t);
                let mut n = 0usize;
                check(&lib, size(t, &mut n), "byte_size")?;
                if n != len * 4 {
                    return Err(format!("тензор {i}: {n} байт, ожидалось {}", len * 4));
                }
                let mut p = null_mut();
                check(&lib, data(t, &mut p), "tensor_data")?;
                ptrs.push(p);
            }

            let mut m = Model {
                infer: lib.get(b"ov_infer_request_infer\0").map_err(|e| e.to_string())?,
                tensor_free: lib.get(b"ov_tensor_free\0").map_err(|e| e.to_string())?,
                request_free: lib.get(b"ov_infer_request_free\0").map_err(|e| e.to_string())?,
                compiled_free: lib.get(b"ov_compiled_model_free\0").map_err(|e| e.to_string())?,
                core_free: lib.get(b"ov_core_free\0").map_err(|e| e.to_string())?,
                _lib: lib,
                core,
                compiled,
                request,
                tensors,
                inputs: input_lens.iter().zip(&ptrs).map(|(&n, p)| (p.cast(), n)).collect(),
                out: (ptrs[input_lens.len()].cast(), output_len),
                device,
            };
            // прогрев: первый прогон на NPU медленный
            let zeros: Vec<Vec<i32>> = input_lens.iter().map(|&n| vec![0; n]).collect();
            let refs: Vec<&[i32]> = zeros.iter().map(Vec::as_slice).collect();
            let mut out = vec![0f32; output_len];
            for _ in 0..3 {
                m.infer(&refs, &mut out)?;
            }
            Ok(m)
        }
    }

    /// Прогон: входы по порядку, результат в out.
    pub fn infer(&mut self, ins: &[&[i32]], out: &mut [f32]) -> Result<(), String> {
        unsafe {
            for (&(p, n), src) in self.inputs.iter().zip(ins) {
                std::ptr::copy_nonoverlapping(src.as_ptr(), p, n.min(src.len()));
            }
            check(&self._lib, (self.infer)(self.request), "infer")?;
            std::ptr::copy_nonoverlapping(self.out.0, out.as_mut_ptr(), self.out.1.min(out.len()));
            Ok(())
        }
    }

    /// Модель раскладки: вероятность «перевернуть» для каждой позиции окна.
    pub fn run(&mut self, tok: &[i32; W], lay: &[i32; W]) -> Result<[f32; W], String> {
        let mut out = [0f32; W];
        self.infer(&[tok, lay], &mut out)?;
        Ok(out)
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        unsafe {
            for &t in &self.tensors {
                (self.tensor_free)(t);
            }
            (self.request_free)(self.request);
            (self.compiled_free)(self.compiled);
            (self.core_free)(self.core);
        }
    }
}
