//! Dart's `Dart_CObject` (`dart_native_api.h` of the Dart SDK Flutter
//! 3.47.7 ships), only as far as the core posts typed data, and the post
//! function's type. Building one and passing a pointer to it is safe Rust;
//! nothing here reads one.

/// `Dart_CObject_kTypedData`.
pub const K_TYPED_DATA: i32 = 7;
/// `Dart_TypedData_kUint8`.
pub const K_UINT8: i32 = 2;

/// `as_typed_data`: a typed list Dart copies when it is posted.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypedData {
    pub ty: i32,
    pub length: isize,
    pub values: *const u8,
}

/// `as_external_typed_data`, the union's largest member; only its size
/// matters here.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExternalTypedData {
    pub ty: i32,
    pub length: isize,
    pub data: *mut u8,
    pub peer: *mut std::ffi::c_void,
    pub callback: *const std::ffi::c_void,
}

/// The value union, sized as the C one by its largest member.
#[repr(C)]
pub union Value {
    pub as_typed_data: TypedData,
    pub as_external_typed_data: ExternalTypedData,
}

/// `Dart_CObject`.
#[repr(C)]
pub struct DartCObject {
    pub ty: i32,
    pub value: Value,
}

/// Dart's `NativeApi.postCObject`: posts `message` to `port` and returns
/// nonzero when it was posted; Dart copies typed data before it returns.
pub type ClaxPost = extern "C" fn(port: i64, message: *mut DartCObject) -> i8;

/// Posts `bytes` to `port` as a Uint8List (Dart copies it); false when the
/// port is closed.
pub fn post_bytes(post: ClaxPost, port: i64, bytes: &[u8]) -> bool {
    let mut obj = DartCObject {
        ty: K_TYPED_DATA,
        value: Value {
            as_typed_data: TypedData {
                ty: K_UINT8,
                length: bytes.len() as isize,
                values: bytes.as_ptr(),
            },
        },
    };
    post(port, &mut obj) != 0
}

/// Dart's post function as a [`crate::Post`].
pub struct DartPost(pub ClaxPost);

impl crate::Post for DartPost {
    fn post(&self, port: i64, bytes: &[u8]) -> bool {
        post_bytes(self.0, port, bytes)
    }
}

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    /// The layout of `Dart_CObject` on a 64-bit target: the type, then
    /// the value union at 8, sized by `as_external_typed_data` (40 bytes).
    #[test]
    fn the_layout_is_dart_cobjects() {
        assert_eq!(size_of::<DartCObject>(), 48);
        assert_eq!(align_of::<DartCObject>(), 8);
        assert_eq!(offset_of!(DartCObject, value), 8);
        assert_eq!(size_of::<Value>(), 40);
        assert_eq!(offset_of!(TypedData, length), 8);
        assert_eq!(offset_of!(TypedData, values), 16);
    }
}
