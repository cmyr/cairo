use std::marker::PhantomData;

use ffi::cairo_user_data_key_t;

/// A key for indexing user data in various cairo types.
///
/// Some types like [`Surface`] have `get_user_data`, `set_user_data`, and `remove_user_data`
/// methods that take `&'static UserDataKey`, where the address of that reference is significant.
///
/// To reliably have a stable address, the expected usage is to define a `static` item:
///
/// ```
/// use cairo::UserDataKey;
/// static FOO: UserDataKey<String> = UserDataKey::new();
///
/// # fn foo(surface: &cairo::Surface) {
/// surface.get_user_data(&FOO)
/// # ; }
pub struct UserDataKey<T> {
    pub(crate) ffi: cairo_user_data_key_t,
    marker: PhantomData<*const T>,
}

unsafe impl<T> Sync for UserDataKey<T> {}

impl<T> UserDataKey<T> {
    pub const fn new() -> Self {
        UserDataKey {
            ffi: cairo_user_data_key_t { unused: 0 },
            marker: PhantomData,
        }
    }
}

// In a safe API for user data we can’t make `get_user_data`
// transfer full ownership of the value to the caller (e.g. by returning `Box<T>`)
// because `self` still has a pointer to that value
// and `get_user_data` could be called again with the same key.
//
// We also can’t return a `&T` reference that borrows from `self`
// because the value could be removed with `remove_user_data` or replaced with `set_user_data`
// while the borrow still needs to be valid.
// (Borrowing with `&mut self` would not help as `Self` can be itself reference-counted.)
//
// Therefore the value must be reference-counted.
//
// We use `Rc` over `Arc` because the types implementing these methods are `!Send` and `!Sync`.
// See <https://github.com/gtk-rs/cairo/issues/256>

macro_rules! user_data_methods {
    ($as_ptr: expr, $ffi_get_user_data: path, $ffi_set_user_data: path,) => {
        /// Attach user data to `self` for the given `key`.
        pub fn set_user_data<T: 'static>(&self, key: &'static crate::UserDataKey<T>,
                                         value: std::rc::Rc<T>)
        {
            unsafe extern "C" fn destructor<T>(ptr: *mut libc::c_void) {
                let ptr: *const T = ptr as _;
                drop(std::rc::Rc::from_raw(ptr))
            }
            // Safety:
            //
            // The destructor’s cast and `from_raw` are symetric
            // with the `into_raw` and cast below.
            // They both transfer ownership of one strong reference:
            // neither of them touches the reference count.
            let ptr: *const T = std::rc::Rc::into_raw(value);
            let ptr = ptr as *mut T as *mut libc::c_void;
            let result = unsafe {
                $ffi_set_user_data($as_ptr(self), &key.ffi, ptr, Some(destructor::<T>))
            };
            Status::from(result).ensure_valid()
        }

        /// Return the user data previously attached to `self` with the given `key`, if any.
        pub fn get_user_data<T: 'static>(&self, key: &'static crate::UserDataKey<T>)
                                         -> Option<std::rc::Rc<T>>
        {
            // Safety:
            //
            // `Rc::from_raw` would normally take ownership of a strong reference for this pointer.
            // But `self` still has a copy of that pointer and `get_user_data` can be called again
            // with the same key.
            // We use `ManuallyDrop` to avoid running the destructor of that first `Rc`,
            // and return a cloned one (which increments the reference count).
            //
            // If `ffi_get_user_data` returns a non-null pointer,
            // there was a previous call to `ffi_set_user_data` with a key with the same address.
            // Either:
            //
            // * This was a call to a Rust `Self::set_user_data` method.
            //   Because that method takes a `&'static` reference,
            //   the key used then must live at that address until the end of the process.
            //   Because `UserDataKey<T>` has a non-zero size regardless of `T`,
            //   no other `UserDataKey<U>` value can have the same address.
            //   Therefore the `T` type was the same then at it is now and `cast` is type-safe.
            //
            // * Or, it is technically possible that the `set` call was to the C function directly,
            //   with a `cairo_user_data_key_t` in heap-allocated memory that was then freed,
            //   then `Box::new(UserDataKey::new()).leak()` was used to create a `&'static`
            //   that happens to have the same address because the allocator for `Box`
            //   reused that memory region.
            //   Since this involves a C (or FFI) call *and* is so far out of “typical” use
            //   of the user data functionality, we consider this a misuse of an unsafe API.
            unsafe {
                let ptr = $ffi_get_user_data($as_ptr(self), &key.ffi);
                let ptr: *mut T = std::ptr::NonNull::new(ptr)?.cast().as_ptr();
                let rc = std::mem::ManuallyDrop::new(std::rc::Rc::from_raw(ptr));
                Some(std::rc::Rc::clone(&rc))
            }
        }

        /// Unattach from `self` the user data associated with `key`, if any.
        /// If there is no other `Rc` strong reference, the data is destroyed.
        pub fn remove_user_data<T: 'static>(&self, key: &'static crate::UserDataKey<T>) {
            let result = unsafe {
                $ffi_set_user_data($as_ptr(self), &key.ffi, std::ptr::null_mut(), None)
            };
            Status::from(result).ensure_valid()
        }
    };
}
