//! JNI bridge for the Android app.
//!
//! Deliberately thin: everything above the audio device already lives in
//! `ausha-client`, so this only marshals four calls and hands the app a slab
//! of float samples. Adding logic here would mean writing it twice when iOS
//! arrives.

use std::ffi::c_void;

use jni::JNIEnv;
use jni::objects::{JClass, JFloatArray, JString, ReleaseMode};
use jni::sys::{jfloat, jint, jlong, jstring};

use ausha_client::{Client, Config, Latency};

/// Handed to Kotlin as an opaque `long`. Kotlin promises to call `fill` from
/// one thread only and to pass the handle back exactly as it was given.
struct Handle {
    client: Client,
}

fn to_handle(pointer: jlong) -> Option<&'static mut Handle> {
    if pointer == 0 {
        return None;
    }
    // Safety: the pointer came from Box::into_raw below and Kotlin holds it
    // until nativeStop, which is the only place it is freed.
    unsafe { (pointer as *mut Handle).as_mut() }
}

fn string_arg(env: &mut JNIEnv, value: &JString) -> String {
    env.get_string(value)
        .map(|s| s.into())
        .unwrap_or_else(|_| String::new())
}

fn throw_io(env: &mut JNIEnv, message: &str) {
    let _ = env.throw_new("java/io/IOException", message);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ausha_receiver_Native_nativeInitLogging(
    _env: JNIEnv,
    _class: JClass,
) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("ausha"),
    );
    log::info!("ausha native logging ready");
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ausha_receiver_Native_nativeConnect(
    mut env: JNIEnv,
    _class: JClass,
    host: JString,
    port: jint,
    token: JString,
    name: JString,
    simulate_loss: jint,
    latency: jint,
) -> jlong {
    let config = Config {
        host: string_arg(&mut env, &host),
        control_port: port.clamp(1, 65535) as u16,
        token: string_arg(&mut env, &token),
        name: string_arg(&mut env, &name),
        simulate_loss: simulate_loss.clamp(0, 100) as u32,
        latency: match latency {
            0 => Latency::Low,
            2 => Latency::Stable,
            _ => Latency::Balanced,
        },
    };
    log::info!("connecting to {}:{}", config.host, config.control_port);

    match Client::connect(&config) {
        Ok(client) => {
            log::info!("connected: {:?}", client.params());
            Box::into_raw(Box::new(Handle { client })) as jlong
        }
        Err(e) => {
            log::warn!("connect failed: {e}");
            throw_io(&mut env, &e.to_string());
            0
        }
    }
}

/// Fills `output` with interleaved float samples and returns how many were
/// real audio rather than silence. Called from the AudioTrack thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ausha_receiver_Native_nativeFill(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    output: JFloatArray,
) -> jint {
    let Some(handle) = to_handle(handle) else {
        return -1;
    };
    // Safety: `output` is a Java float array the caller owns for the duration
    // of this call; CopyBack writes the samples back on drop.
    let mut elements = match unsafe { env.get_array_elements(&output, ReleaseMode::CopyBack) } {
        Ok(elements) => elements,
        Err(_) => return -1,
    };
    let samples: &mut [f32] = unsafe {
        std::slice::from_raw_parts_mut(elements.as_mut_ptr() as *mut jfloat, elements.len())
    };
    handle.client.fill(samples).filled as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ausha_receiver_Native_nativeStats(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let json = match to_handle(handle) {
        Some(handle) => serde_json::to_string(&handle.client.stats()).unwrap_or_default(),
        None => String::new(),
    };
    env.new_string(json)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut() as jstring)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ausha_receiver_Native_nativeIsRunning(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    match to_handle(handle) {
        Some(handle) if handle.client.is_running() => 1,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_ausha_receiver_Native_nativeDisconnect(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    // Safety: takes back the box created in nativeConnect. Kotlin must not use
    // the handle afterwards, which AushaEngine enforces by nulling it.
    let handle = unsafe { Box::from_raw(handle as *mut Handle) };
    log::info!("disconnecting");
    drop(handle);
}

/// Keeps the linker from dropping the library when nothing else references it.
#[unsafe(no_mangle)]
pub extern "system" fn JNI_OnLoad(_vm: *mut c_void, _reserved: *mut c_void) -> jint {
    jni::sys::JNI_VERSION_1_6
}
