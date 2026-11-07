//! Windows burning through IMAPI2, called from Rust.
//!
//! The interfaces are all IDispatch based, so everything below is late bound by member
//! name: no generated interop, no scripting host. Progress is the one thing that genuinely
//! needs COM plumbing - \`DDiscFormat2DataEvents::Update\` is a dispinterface (IDL: DISPID
//! 0x200, called every half second while writing), and .NET cannot implement one by hand,
//! which is why this lives here in Rust rather than in a helper script.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

type HResult = i32;
type BStr = *const u16;

const S_OK: HResult = 0;

const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;
const VT_DISPATCH: u16 = 9;
const VT_BOOL: u16 = 11;
const VT_UNKNOWN: u16 = 13;
const VT_ARRAY: u16 = 0x2000;
const VT_VARIANT: u16 = 12;

const DISPATCH_METHOD: u16 = 1;
const DISPATCH_PROPERTYGET: u16 = 2;
const DISPATCH_PROPERTYPUT: u16 = 4;

const IID_DISPATCH: Guid = guid(0x0002_0400, 0x0000, 0x0000, [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46]);
const CLSID_DISC_MASTER: Guid = guid(0x2735_412E, 0x7F64, 0x5B0F, [0x8F, 0x00, 0x5D, 0x77, 0xAF, 0xBE, 0x26, 0x1E]);
const CLSID_DISC_RECORDER: Guid = guid(0x2735_412D, 0x7F64, 0x5B0F, [0x8F, 0x00, 0x5D, 0x77, 0xAF, 0xBE, 0x26, 0x1E]);
const CLSID_FORMAT2_DATA: Guid = guid(0x2735_412A, 0x7F64, 0x5B0F, [0x8F, 0x00, 0x5D, 0x77, 0xAF, 0xBE, 0x26, 0x1E]);
/// DDiscFormat2DataEvents, from imapi2.idl.
const IID_DATA_EVENTS: Guid = guid(0x2735_413C, 0x7F64, 0x5B0F, [0x8F, 0x00, 0x5D, 0x77, 0xAF, 0xBE, 0x26, 0x1E]);
const IID_CONNECTION_POINT_CONTAINER: Guid = guid(0xB196_B284, 0xBAB4, 0x101A, [0xB6, 0x9C, 0x00, 0xAA, 0x00, 0x34, 0x1D, 0x07]);
const DISPID_UPDATE: i32 = 0x200;

/// The same GUID layout windows-sys uses, so the values can go straight to CoCreateInstance.
use windows_sys::core::GUID as Guid;

const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Guid {
    Guid { data1, data2, data3, data4 }
}

/// A COM VARIANT, 24 bytes as the ABI defines it. Written out here so the layout is explicit.
#[repr(C)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    data: [u64; 2],
}

impl Variant {
    fn empty() -> Self {
        Variant { vt: VT_EMPTY, reserved: [0; 3], data: [0; 2] }
    }
    fn from_i32(v: i32) -> Self {
        let mut out = Variant::empty();
        out.vt = VT_I4;
        out.data[0] = v as u32 as u64;
        out
    }
    fn from_bool(v: bool) -> Self {
        let mut out = Variant::empty();
        out.vt = VT_BOOL;
        out.data[0] = if v { 0xFFFF } else { 0 };
        out
    }
    fn from_bstr(text: &str) -> Result<Self, String> {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let b = unsafe { SysAllocString(wide.as_ptr()) };
        if b.is_null() {
            return Err("无法分配字符串".into());
        }
        let mut out = Variant::empty();
        out.vt = VT_BSTR;
        out.data[0] = b as u64;
        Ok(out)
    }
    /// Holds a reference of its own: VariantClear releases it when the Variant goes away.
    fn from_unknown(ptr: *mut c_void) -> Self {
        unsafe {
            let d = &*(ptr as *const Disp);
            ((*d.vtbl).add_ref)(ptr);
        }
        let mut out = Variant::empty();
        out.vt = VT_UNKNOWN;
        out.data[0] = ptr as u64;
        out
    }
    fn as_i64(&self) -> Option<i64> {
        match self.vt {
            VT_I4 => Some(self.data[0] as u32 as i32 as i64),
            VT_BOOL => Some(((self.data[0] & 0xFFFF) != 0) as i64),
            _ => None,
        }
    }
    fn dispatch(&self) -> Option<*mut c_void> {
        match self.vt {
            VT_DISPATCH | VT_UNKNOWN => {
                let p = self.data[0] as *mut c_void;
                (!p.is_null()).then_some(p)
            }
            _ => None,
        }
    }
}

impl Drop for Variant {
    fn drop(&mut self) {
        unsafe { VariantClear(self as *mut Variant) };
    }
}

#[repr(C)]
struct DispParams {
    args: *mut Variant,
    named: *mut i32,
    arg_count: u32,
    named_count: u32,
}

#[repr(C)]
struct DispVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_type_info_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HResult,
    get_type_info: unsafe extern "system" fn(*mut c_void, u32, u32, *mut *mut c_void) -> HResult,
    get_ids_of_names: unsafe extern "system" fn(*mut c_void, *const Guid, *const BStr, u32, u32, *mut i32) -> HResult,
    invoke: unsafe extern "system" fn(*mut c_void, i32, *const Guid, u32, u16, *mut DispParams, *mut Variant, *mut c_void, *mut u32) -> HResult,
}

#[repr(C)]
struct Disp {
    vtbl: *const DispVtbl,
}

#[repr(C)]
struct ConnectionPointVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_connection_interface: unsafe extern "system" fn(*mut c_void, *mut Guid) -> HResult,
    get_connection_point_container: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult,
    advise: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut u32) -> HResult,
}

#[repr(C)]
struct ConnectionPoint {
    vtbl: *const ConnectionPointVtbl,
}

#[repr(C)]
struct ConnectionContainerVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    enum_connection_points: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult,
    find_connection_point: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut ConnectionPoint) -> HResult,
}

#[repr(C)]
struct ConnectionContainer {
    vtbl: *const ConnectionContainerVtbl,
}

#[link(name = "oleaut32")]
extern "system" {
    fn SysAllocString(src: *const u16) -> BStr;
    fn VariantClear(v: *mut Variant) -> HResult;
    fn SafeArrayGetLBound(array: *mut c_void, dim: u32, out: *mut i32) -> HResult;
    fn SafeArrayGetUBound(array: *mut c_void, dim: u32, out: *mut i32) -> HResult;
    fn SafeArrayAccessData(array: *mut c_void, out: *mut *mut c_void) -> HResult;
    fn SafeArrayUnaccessData(array: *mut c_void) -> HResult;
}

// Hands the image to IMAPI. The declaration that works is the native one: a managed stream
// is refused with E_IMAPI_DF2DATA_STREAM_NOT_SUPPORTED (0xC0AA0403).
#[link(name = "shlwapi")]
extern "system" {
    fn SHCreateStreamOnFileEx(file: *const u16, mode: u32, attributes: u32, create: i32, template: *mut c_void, out: *mut *mut c_void) -> HResult;
}

/// A late-bound COM object. Releases itself, so callers never track lifetimes.
struct Object(*mut c_void);

// IMAPI2 objects are registered ThreadingModel=Free; the burn thread and the watcher share them.
unsafe impl Send for Object {}
unsafe impl Sync for Object {}

impl Object {
    unsafe fn dispatch(&self) -> &Disp {
        &*(self.0 as *const Disp)
    }

    fn id_of(&self, name: &str) -> Result<i32, String> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut ids = [0i32; 1];
        let hr = unsafe {
            let d = self.dispatch();
            let names: [BStr; 1] = [wide.as_ptr()];
            ((*d.vtbl).get_ids_of_names)(self.0, &IID_NULL, names.as_ptr(), 1, 0x0409, ids.as_mut_ptr())
        };
        if hr < 0 {
            return Err(format!("{name}：找不到成员 (0x{:08X})", hr as u32));
        }
        Ok(ids[0])
    }

    fn invoke(&self, id: i32, flags: u16, args: &mut Vec<Variant>) -> Result<Variant, String> {
        self.invoke_with(id, flags, args, None)
    }

    fn invoke_with(
        &self,
        id: i32,
        flags: u16,
        args: &mut Vec<Variant>,
        mut named: Option<Vec<i32>>,
    ) -> Result<Variant, String> {
        // COM wants the arguments in reverse order.
        args.reverse();
        let mut params = DispParams {
            args: args.as_mut_ptr(),
            named: named.as_mut().map_or(null_mut(), |n| n.as_mut_ptr()),
            arg_count: args.len() as u32,
            named_count: named.as_ref().map_or(0, |n| n.len() as u32),
        };
        let mut result = Variant::empty();
        let hr = unsafe {
            let d = self.dispatch();
            ((*d.vtbl).invoke)(self.0, id, &IID_NULL, 0x0409, flags, &mut params, &mut result, null_mut(), null_mut())
        };
        if hr < 0 {
            return Err(format!("0x{:08X}", hr as u32));
        }
        Ok(result)
    }

    fn get(&self, name: &str) -> Result<Variant, String> {
        let id = self.id_of(name)?;
        self.invoke(id, DISPATCH_PROPERTYGET, &mut Vec::new())
    }

    fn number(&self, name: &str) -> Result<i64, String> {
        self.get(name)?.as_i64().ok_or_else(|| format!("{name} 不是数字"))
    }

    fn put(&self, name: &str, value: Variant) -> Result<(), String> {
        let id = self.id_of(name)?;
        let mut args = vec![value];
        // A property put carries DISPID_PROPERTYPUT as its one named argument.
        self.invoke_with(id, DISPATCH_PROPERTYPUT, &mut args, Some(vec![DISPID_PROPERTYPUT]))
            .map(|_| ())
    }

    fn call(&self, name: &str, mut args: Vec<Variant>) -> Result<Variant, String> {
        let id = self.id_of(name)?;
        self.invoke(id, DISPATCH_METHOD, &mut args)
    }

    fn strings(&self, name: &str) -> Result<Vec<String>, String> {
        let value = self.get(name)?;
        if value.vt != (VT_ARRAY | VT_VARIANT) {
            return Ok(Vec::new());
        }
        let array = value.data[0] as *mut c_void;
        if array.is_null() {
            return Ok(Vec::new());
        }
        unsafe {
            let (mut low, mut high) = (0i32, -1i32);
            if SafeArrayGetLBound(array, 1, &mut low) < 0 || SafeArrayGetUBound(array, 1, &mut high) < 0 {
                return Ok(Vec::new());
            }
            let mut raw: *mut c_void = null_mut();
            if SafeArrayAccessData(array, &mut raw) < 0 {
                return Ok(Vec::new());
            }
            let items = raw as *const Variant;
            let mut out = Vec::new();
            for i in 0..=(high - low).max(-1) {
                let item = &*items.add(i as usize);
                if item.vt == VT_BSTR {
                    out.push(read_bstr(item.data[0] as BStr));
                }
            }
            SafeArrayUnaccessData(array);
            Ok(out)
        }
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        unsafe {
            let d = self.dispatch();
            ((*d.vtbl).release)(self.0);
        }
    }
}

const IID_NULL: Guid = guid(0, 0, 0, [0; 8]);

unsafe fn read_bstr(b: BStr) -> String {
    if b.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *b.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(b, len))
}
/// IMAPI_FORMAT2_DATA_WRITE_ACTION, from imapi2.idl.
fn phase_name(action: i64) -> &'static str {
    match action {
        0 => "校验介质",
        1 => "格式化介质",
        2 => "初始化光驱",
        3 => "校准激光功率",
        4 => "写入数据",
        5 => "封盘",
        6 => "写入完成",
        7 => "校验刻录结果",
        _ => "写入中",
    }
}

const DISPID_PROPERTYPUT: i32 = -3;

/// The event sink. DDiscFormat2DataEvents is a dispinterface, so there is no vtable to
/// implement: the object answers IDispatch (and the event IID itself, which is what the
/// connection point asks for) and handles DISPID 0x200.
#[repr(C)]
struct SinkVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_type_info_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HResult,
    get_type_info: unsafe extern "system" fn(*mut c_void, u32, u32, *mut *mut c_void) -> HResult,
    get_ids_of_names: unsafe extern "system" fn(*mut c_void, *const Guid, *const BStr, u32, u32, *mut i32) -> HResult,
    invoke: unsafe extern "system" fn(*mut c_void, i32, *const Guid, u32, u16, *mut DispParams, *mut Variant, *mut c_void, *mut u32) -> HResult,
}

#[repr(C)]
struct Sink {
    vtbl: *const SinkVtbl,
    refs: std::sync::atomic::AtomicU32,
    on_progress: Arc<dyn Fn(f64, &str) + Send + Sync>,
}

impl Sink {
    fn new(on_progress: Arc<dyn Fn(f64, &str) + Send + Sync>) -> Box<Sink> {
        Box::new(Sink { vtbl: &SINK_VTBL, refs: std::sync::atomic::AtomicU32::new(1), on_progress })
    }
}

static SINK_VTBL: SinkVtbl = SinkVtbl {
    query_interface: sink_query_interface,
    add_ref: sink_add_ref,
    release: sink_release,
    get_type_info_count: sink_get_type_info_count,
    get_type_info: sink_get_type_info,
    get_ids_of_names: sink_get_ids_of_names,
    invoke: sink_invoke,
};

unsafe extern "system" fn sink_query_interface(this: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    let wanted = *iid;
    let same = |g: &Guid| {
        wanted.data1 == g.data1 && wanted.data2 == g.data2 && wanted.data3 == g.data3 && wanted.data4 == g.data4
    };
    if same(&IID_UNKNOWN) || same(&IID_DISPATCH) || same(&IID_DATA_EVENTS) {
        sink_add_ref(this);
        *out = this;
        S_OK
    } else {
        *out = null_mut();
        0x8000_4002u32 as i32 // E_NOINTERFACE
    }
}

unsafe extern "system" fn sink_add_ref(this: *mut c_void) -> u32 {
    let sink = &*(this as *const Sink);
    sink.refs.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn sink_release(this: *mut c_void) -> u32 {
    let sink = &*(this as *const Sink);
    let left = sink.refs.fetch_sub(1, Ordering::SeqCst) - 1;
    if left == 0 {
        drop(Box::from_raw(this as *mut Sink));
    }
    left
}

unsafe extern "system" fn sink_get_type_info_count(_this: *mut c_void, count: *mut u32) -> HResult {
    *count = 0;
    S_OK
}

unsafe extern "system" fn sink_get_type_info(_this: *mut c_void, _index: u32, _lcid: u32, info: *mut *mut c_void) -> HResult {
    *info = null_mut();
    0x8000_4004u32 as i32 // E_NOTIMPL
}

unsafe extern "system" fn sink_get_ids_of_names(
    _this: *mut c_void,
    _iid: *const Guid,
    _names: *const BStr,
    _count: u32,
    _lcid: u32,
    ids: *mut i32,
) -> HResult {
    *ids = -1;
    DISP_E_UNKNOWNNAME
}

unsafe extern "system" fn sink_invoke(
    this: *mut c_void,
    id: i32,
    _iid: *const Guid,
    _lcid: u32,
    _flags: u16,
    params: *mut DispParams,
    _result: *mut Variant,
    _except: *mut c_void,
    _argerr: *mut u32,
) -> HResult {
    if id != DISPID_UPDATE || params.is_null() || (*params).arg_count < 1 {
        return S_OK;
    }
    let sink = &*(this as *const Sink);
    trace("event: enter");
    // Arguments arrive in reverse order: the progress object is the last declared one.
    let first = &*(*params).args;
    trace(&format!("event: vt={}", first.vt));
    let Some(pointer) = first.dispatch() else { return S_OK };
    trace("event: borrowed");
    let progress = Object::borrowed(pointer);
    let written = progress.number("LastWrittenLba").unwrap_or(0);
    trace(&format!("event: written={written}"));
    let total = progress.number("SectorCount").unwrap_or(0);
    trace(&format!("event: total={total}"));
    // CurrentAction is IMAPI_FORMAT2_DATA_WRITE_ACTION, an integer, not a string.
    let action = progress.number("CurrentAction").map(phase_name).unwrap_or("写入中");
    trace(&format!("event: action={action}"));
    if total > 0 {
        let percent = (written as f64 * 100.0 / total as f64).clamp(0.0, 99.0);
        (sink.on_progress)(percent, &action);
    }
    S_OK
}

const IID_UNKNOWN: Guid = guid(0x0000_0000, 0x0000, 0x0000, [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46]);
const DISP_E_UNKNOWNNAME: HResult = 0x8002_0006u32 as i32;

impl Object {
    /// Wraps a pointer somebody else owns, taking a reference of our own.
    fn borrowed(pointer: *mut c_void) -> Object {
        unsafe {
            let d = &*(pointer as *const Disp);
            ((*d.vtbl).add_ref)(pointer);
        }
        Object(pointer)
    }

    fn create(class: &Guid) -> Result<Object, String> {
        unsafe {
            CoInitializeEx(null(), COINIT_MULTITHREADED as u32);
            let mut pointer: *mut c_void = null_mut();
            let hr = CoCreateInstance(class, null_mut(), CLSCTX_INPROC_SERVER as u32, &IID_DISPATCH, &mut pointer);
            if hr < 0 || pointer.is_null() {
                return Err(format!("无法创建 IMAPI2 对象 (0x{:08X})", hr as u32));
            }
            Ok(Object(pointer))
        }
    }

    /// A property that takes an index, as the IMAPI collections expose.
    fn indexed(&self, name: &str, index: i32) -> Result<Variant, String> {
        let id = self.id_of(name)?;
        let mut args = vec![Variant::from_i32(index)];
        self.invoke(id, DISPATCH_PROPERTYGET, &mut args)
    }
}
/// Writes a step to a file when tracing is on: an access violation leaves nothing on
/// stderr, so the trail has to be on disk already.
fn trace(step: &str) {
    use std::io::Write;
    if std::env::var("DISCRATCHER_TRACE").is_err() {
        return;
    }
    let path = std::env::temp_dir().join("discratcher-imapi.trace");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{step}");
    }
}

/// Finds the recorder whose volume is this drive, so the rest can be set up against it.
fn find_recorder(drive: &str) -> Result<Object, String> {
    trace("create master");
    let master = Object::create(&CLSID_DISC_MASTER)?;
    trace("count");
    let count = master.number("Count")?;
    trace(&format!("count={count}"));
    let wanted = format!("{drive}\\");
    for i in 0..count {
        trace(&format!("item {i}"));
        let value = master.indexed("Item", i as i32)?;
        trace("read id");
        let id = unsafe { read_bstr(value.data[0] as BStr) };
        trace(&format!("id={id}"));
        let recorder = Object::create(&CLSID_DISC_RECORDER)?;
        trace("init recorder");
        recorder.call("InitializeDiscRecorder", vec![Variant::from_bstr(&id)?])?;
        trace("volumes");
        for volume in recorder.strings("VolumePathNames").unwrap_or_default() {
            trace(&format!("volume={volume}"));
            if volume.eq_ignore_ascii_case(&wanted) {
                return Ok(recorder);
            }
        }
    }
    Err(format!("找不到 {drive} 对应的刻录机"))
}

/// Burn an image to a drive. The callback runs on IMAPI's own thread, up to twice a second,
/// with the percentage and the phase name the drive reports.
pub fn burn(
    drive: &str,
    iso: &str,
    speed: u8,
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(f64, &str) + Send + Sync + 'static,
) -> Result<(), String> {
    let reporter: Arc<dyn Fn(f64, &str) + Send + Sync> = Arc::new(on_progress);
    let recorder = find_recorder(drive)?;

    let format = Object::create(&CLSID_FORMAT2_DATA)?;
    format.put("Recorder", Variant::from_unknown(recorder.0))?;
    format.put("ClientName", Variant::from_bstr("Discratcher")?)?;
    let _ = format.put("ForceMediaToBeClosed", Variant::from_bool(true));

    if speed > 0 {
        // Sectors per second, which is the unit IMAPI2 reports and takes: one DVD speed is
        // 692.5 of them, one CD speed is 75. Rotation type follows the drive default.
        let media = format.number("CurrentPhysicalMediaType").unwrap_or(0);
        let per_speed = if (1..=3).contains(&media) { 75.0 } else { 692.5 };
        let sectors_per_second = (speed as f64 * per_speed).round() as i32;
        format.call("SetWriteSpeed", vec![Variant::from_i32(sectors_per_second), Variant::from_bool(true)])?;
        if let Ok(current) = format.number("CurrentWriteSpeed") {
            reporter(0.0, &format!("写入速度 {current} 扇区/秒"));
        }
    }

    if !bool_of(format.call("IsRecorderSupported", vec![Variant::from_unknown(recorder.0)])?)? {
        return Err("这台刻录机不受 Windows 刻录接口支持".into());
    }
    if !bool_of(format.call("IsCurrentMediaSupported", vec![Variant::from_unknown(recorder.0)])?)? {
        return Err("这张盘不能写入".into());
    }

    let wide: Vec<u16> = iso.encode_utf16().chain(std::iter::once(0)).collect();
    let mut stream: *mut c_void = null_mut();
    let hr = unsafe { SHCreateStreamOnFileEx(wide.as_ptr(), 0, 0x80, 0, null_mut(), &mut stream) };
    if hr < 0 || stream.is_null() {
        return Err(format!("无法打开镜像 (0x{:08X})", hr as u32));
    }

    let sink = Box::into_raw(Sink::new(reporter.clone()));
    advice(&format, unsafe { &*sink })?;

    let shared = Arc::new(format);
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (shared, stop, cancel) = (shared.clone(), stop.clone(), cancel.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    let _ = shared.call("CancelWrite", Vec::new());
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        })
    };

    trace("write: begin");
    let written = shared.call("Write", vec![Variant::from_unknown(stream)]);
    trace("write: returned");
    stop.store(true, Ordering::Relaxed);
    let _ = watcher.join();

    if cancel.load(Ordering::Relaxed) {
        return Err("已取消".into());
    }
    written.map_err(|e| format!("刻录失败：0x{e}"))?;
    reporter(100.0, "刻录完成");
    Ok(())
}

fn bool_of(value: Variant) -> Result<bool, String> {
    Ok(value.as_i64().unwrap_or(0) != 0)
}

/// Asks an object for one of its interfaces.
unsafe fn query<T>(pointer: *mut c_void, iid: &Guid) -> Result<*mut T, String> {
    let d = &*(pointer as *const Disp);
    let mut out: *mut c_void = null_mut();
    let hr = ((*d.vtbl).query_interface)(pointer, iid, &mut out);
    if hr < 0 || out.is_null() {
        return Err(format!("接口不可用 (0x{:08X})", hr as u32));
    }
    Ok(out as *mut T)
}

/// Connects the sink to the burn object's event source.
fn advice(format: &Object, sink: &Sink) -> Result<(), String> {
    unsafe {
        let container: *mut ConnectionContainer = query(format.0, &IID_CONNECTION_POINT_CONTAINER)?;
        let mut point: *mut ConnectionPoint = null_mut();
        let hr = ((*(*container).vtbl).find_connection_point)(container as *mut c_void, &IID_DATA_EVENTS, &mut point);
        if hr < 0 || point.is_null() {
            ((*(*container).vtbl).release)(container as *mut c_void);
            return Err(format!("找不到进度事件接口 (0x{:08X})", hr as u32));
        }
        let mut cookie = 0u32;
        let hr = ((*(*point).vtbl).advise)(point as *mut c_void, sink as *const Sink as *mut c_void, &mut cookie);
        ((*(*point).vtbl).release)(point as *mut c_void);
        ((*(*container).vtbl).release)(container as *mut c_void);
        if hr < 0 {
            return Err(format!("挂接进度事件失败 (0x{:08X})", hr as u32));
        }
        Ok(())
    }
}

/// Everything the burn does up to the write itself, so a crash there can be found without
/// spending a disc.
pub fn probe_burn_setup(drive: &str, iso: &str, speed: u8) -> Result<String, String> {
    let recorder = find_recorder(drive)?;
    trace("setup: format");
    let format = Object::create(&CLSID_FORMAT2_DATA)?;
    trace("setup: recorder");
    format.put("Recorder", Variant::from_unknown(recorder.0))?;
    trace("setup: client");
    format.put("ClientName", Variant::from_bstr("Discratcher")?)?;
    trace("setup: close media");
    format.put("ForceMediaToBeClosed", Variant::from_bool(true))?;
    trace("setup: media type");
    let media = format.number("CurrentPhysicalMediaType").unwrap_or(0);
    let per_speed = if (1..=3).contains(&media) { 75.0 } else { 692.5 };
    let sectors = (speed as f64 * per_speed).round() as i32;
    trace(&format!("setup: set speed {sectors}"));
    format.call("SetWriteSpeed", vec![Variant::from_i32(sectors), Variant::from_bool(true)])?;
    let current = format.number("CurrentWriteSpeed").unwrap_or(0);
    trace("setup: supported");
    let supported = bool_of(format.call("IsRecorderSupported", vec![Variant::from_unknown(recorder.0)])?).unwrap_or(false);
    let media_ok = bool_of(format.call("IsCurrentMediaSupported", vec![Variant::from_unknown(recorder.0)])?).unwrap_or(false);
    trace("setup: stream");
    let wide: Vec<u16> = iso.encode_utf16().chain(std::iter::once(0)).collect();
    let mut stream: *mut c_void = null_mut();
    let hr = unsafe { SHCreateStreamOnFileEx(wide.as_ptr(), 0, 0x80, 0, null_mut(), &mut stream) };
    trace("setup: advice");
    let sink = Box::into_raw(Sink::new(Arc::new(|_p: f64, _a: &str| {})));
    let events = advice(&format, unsafe { &*sink });
    Ok(format!(
        "media_type={media} speed={sectors} current={current} supported={supported} media_ok={media_ok} stream_hr=0x{hr:08X} events={}",
        match events { Ok(()) => "ok".to_string(), Err(e) => e }
    ))
}

/// What the drive says about itself, without burning anything: the self test for the COM
/// plumbing, which is all that can be checked without spending a disc.
pub fn probe(drive: &str) -> Result<String, String> {
    let recorder = find_recorder(drive)?;
    let format = Object::create(&CLSID_FORMAT2_DATA)?;
    format.put("Recorder", Variant::from_unknown(recorder.0))?;
    let media = bool_of(format.call("IsCurrentMediaSupported", vec![Variant::from_unknown(recorder.0)])?).unwrap_or(false);
    let total = format.number("TotalSectorsOnMedia").unwrap_or(0);
    // Attaching the event sink needs no media, so this part of the burn is checked here
    // rather than discovered halfway through a disc.
    let sink = Sink::new(Arc::new(|_percent: f64, _phase: &str| {}));
    let events = match advice(&format, &sink) {
        Ok(()) => "ok".to_string(),
        Err(e) => e,
    };
    Ok(format!("recorder=ok media_supported={media} total_sectors={total} events={events}"))
}