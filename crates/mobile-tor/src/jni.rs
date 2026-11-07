//! JNI for Kotlin: `expo.modules.obtor.ObTorNative`.
//! Errors come back as a `java.io.IOException`.

use std::path::Path;
use std::time::Duration;

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jbyteArray, jint, jlong, jstring};
use jni::JNIEnv;

fn read(env: &mut JNIEnv, value: &JString) -> Result<String, String> {
    env.get_string(value)
        .map(String::from)
        .map_err(|err| err.to_string())
}

fn fail(env: &mut JNIEnv, err: String) {
    let _ = env.throw_new("java/io/IOException", err);
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_obtor_ObTorNative_start(
    mut env: JNIEnv,
    _: JClass,
    data_dir: JString,
) {
    if let Err(err) = read(&mut env, &data_dir).and_then(|dir| crate::start(Path::new(&dir))) {
        fail(&mut env, err);
    }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_obtor_ObTorNative_status(
    env: JNIEnv,
    _: JClass,
) -> jstring {
    env.new_string(crate::status().to_json())
        .map(|value| value.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_obtor_ObTorNative_insertDiscoveryKey(
    mut env: JNIEnv,
    _: JClass,
    onion: JString,
    secret: JString,
) {
    let result = read(&mut env, &onion).and_then(|onion| {
        let secret = read(&mut env, &secret)?;
        crate::insert_discovery_key(&onion, &secret)
    });
    if let Err(err) = result {
        fail(&mut env, err);
    }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_obtor_ObTorNative_exchange(
    mut env: JNIEnv,
    _: JClass,
    onion: JString,
    port: jint,
    request: JByteArray,
    max_bytes: jint,
    timeout_ms: jlong,
) -> jbyteArray {
    let result = read(&mut env, &onion).and_then(|onion| {
        let request = env
            .convert_byte_array(&request)
            .map_err(|err| err.to_string())?;
        let port = u16::try_from(port).map_err(|_| "bad port".to_string())?;
        crate::exchange(
            &onion,
            port,
            &request,
            max_bytes.max(0) as usize,
            Duration::from_millis(timeout_ms.max(0) as u64),
        )
    });
    match result.and_then(|bytes| {
        env.byte_array_from_slice(&bytes)
            .map_err(|err| err.to_string())
    }) {
        Ok(array) => array.into_raw(),
        Err(err) => {
            fail(&mut env, err);
            std::ptr::null_mut()
        }
    }
}
