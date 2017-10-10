//! Server-side Wayland connector
//!
//! # Overview
//!
//! Setting up the listening socket is done by the `create_display`
//! function, providing you a `Display` object and an `EventLoop`.
//!
//! On the event loop, you'll be able to register the globals
//! you want to advertize, as well as handlers for all ressources
//! created by the clients.
//!
//! You then integrate the wayland event loop in your main event
//! loop to run your compositor.
//!
//! # Implementation and event loop
//!
//! This crate mirrors the callback-oriented design of the
//! Wayland C library by using implementation structs: each wayland
//! type defines an `Implementation` struct in its module, with
//! one function field for each possible event this object can receive.
//!
//! When registering an object on an event loop, you need to provide an
//! implementation for this object. You can also provide some
//! "implementation data": a value that will be provided as second
//! argument to all the callback methods of your implementation.
//!
//! A typical use of implementation data is to store here one or more
//! state tokens to access some part of the shared state from your
//! callback.
//!
//! ## Example of implementation
//!
//! You can register your wayland objects to an event queue:
//!
//! ```ignore
//! event_loop.register(&my_object, implementation, impl_data);
//! ```
//!
//! A given wayland object can only be registered to an event
//! loop at a given time, re-registering it will overwrite
//! the previous configuration.
//!
//! Objects can be registered to event loop using the `&EventLoopHandle`
//! argument, available from withing an event callback.
//!
//! ## Globals definition
//!
//! Some wayland objects are special and can be directly created by the
//! clients from their registry. To handle them your must declare
//! which globals you want to make available to your clients, like this:
//!
//! ```ignore
//! event_loop.register_global(version, callback, idata);
//! ```
//!
//! Where `callback` is a function or non-capturing closure, provided as
//! an implementation for when this global is instanciated by a client.
//! See the method documentation for details.
//!
//! ## Event loop integration
//!
//! Once the setup phase is done, you can integrate the
//! event loop in the main event loop of your program.
//!
//! Either all you need is for it to run indefinitely (external
//! events are checked in an other thread?):
//!
//! ```ignore
//! event_loop.run();
//! ```
//!
//! Or you can integrate it with more control:
//!
//! ```ignore
//! loop {
//!     // flush events to client sockets
//!     display.flush_clients();
//!     // receive request from clients and dispatch them
//!     // blocking if no request is pending for at most
//!     // 10ms
//!     event_loop.dispatch(Some(10)).unwrap();
//!     // then you can check events from other sources if
//!     // you need to
//! }
//! ```
//!
//! # Protocols integration
//!
//! This crate provides the basic primitives as well as the
//! core wayland protocol (in the `protocol` module), but
//! other protocols can be integrated from XML descriptions.
//!
//! The the crate `wayland_scanner` and its documentation for
//! details about how to do so.

#![warn(missing_docs)]

#[macro_use]
extern crate bitflags;
extern crate libc;
extern crate nix;
extern crate token_store;
#[macro_use]
extern crate wayland_sys;

pub use client::Client;
pub use display::{create_display, Display};
pub use event_loop::{resource_is_registered, EventLoop, EventLoopHandle, Global, GlobalCallback,
                     RegisterStatus, State, StateToken};
pub use generated::interfaces as protocol_interfaces;
pub use generated::server as protocol;
use wayland_sys::common::{wl_argument, wl_interface};
use wayland_sys::server::*;

mod client;
mod display;
mod event_loop;
mod event_sources;

pub mod sources {
    //! Secondary event sources
    // This module contains the types & traits to work with
    // different kind of event sources that can be registered to and
    // event loop, other than the wayland protocol sockets.

    pub use event_sources::{FdEventSource, FdEventSourceImpl, FdInterest};
    pub use event_sources::{SignalEventSource, SignalEventSourceImpl};
    pub use event_sources::{TimerEventSource, TimerEventSourceImpl};
}

/// Common routines for wayland resource objects.
///
/// All wayland objects automatically implement this trait
/// as generated by the scanner.
///
/// It is mostly used for internal use by the library, and you
/// should only need these methods for interfacing with C library
/// working on wayland objects.
pub unsafe trait Resource {
    /// Pointer to the underlying wayland proxy object
    fn ptr(&self) -> *mut wl_resource;
    /// Create an instance from a wayland pointer
    ///
    /// The pointer must refer to a valid wayland resource
    /// of the appropriate interface, but that have not yet
    /// been seen by the library.
    ///
    /// The library will take control of the object (notably
    /// overwrite its user_data).
    unsafe fn from_ptr_new(*mut wl_resource) -> Self;
    /// Create an instance from a wayland pointer
    ///
    /// The pointer must refer to a valid wayland resource
    /// of the appropriate interface. The library will detect if the
    /// resource is already managed by it or not. If it is not, this
    /// resource will be considered as "unmanaged", and should then
    /// be handled with care.
    unsafe fn from_ptr_initialized(*mut wl_resource) -> Self;
    /// Pointer to the interface representation
    fn interface_ptr() -> *const wl_interface;
    /// Internal wayland name of this interface
    fn interface_name() -> &'static str;
    /// Max version of this interface supported
    fn supported_version() -> u32;
    /// Current version of the interface this resource is instantiated with
    fn version(&self) -> i32;
    /// Check if the resource behind this handle is actually still alive
    fn status(&self) -> Liveness;
    /// Check of two handles are actually the same wayland object
    ///
    /// Returns `false` if any of the objects has already been destroyed
    fn equals(&self, &Self) -> bool;
    /// Set a pointer associated as user data on this resource
    ///
    /// All handles to the same wayland object share the same user data pointer.
    ///
    /// The get/set operations are atomic, no more guarantee is given. If you need
    /// to synchronise access to this data, it is your responsibility to add a Mutex
    /// or any other similar mechanism.
    fn set_user_data(&self, ptr: *mut ());
    /// Get the pointer associated as user data on this resource
    ///
    /// All handles to the same wayland object share the same user data pointer.
    ///
    /// See `set_user_data` for synchronisation guarantee.
    fn get_user_data(&self) -> *mut ();
    /// Posts a protocol error to this resource
    ///
    /// The error code can be obtained from the various `Error` enums of the protocols.
    ///
    /// An error is fatal to the client that caused it.
    fn post_error(&self, error_code: u32, msg: String) {
        // If `str` contains an interior null, the actuall transmitted message will
        // be truncated at this point.
        unsafe {
            let cstring = ::std::ffi::CString::from_vec_unchecked(msg.into());
            ffi_dispatch!(
                WAYLAND_SERVER_HANDLE,
                wl_resource_post_error,
                self.ptr(),
                error_code,
                cstring.as_ptr()
            )
        }
    }
    /// Clone this resource handle
    ///
    /// Will only succeed if the resource is managed by this library and
    /// is still alive.
    fn clone(&self) -> Option<Self>
    where
        Self: Sized,
    {
        if self.status() == Liveness::Alive {
            Some(unsafe { self.clone_unchecked() })
        } else {
            None
        }
    }
    /// Unsafely clone this resource handle
    ///
    /// This function is unsafe because if the resource is unmanaged, the lib
    /// has no knowledge of its lifetime, and cannot ensure that the new handle
    /// will not outlive the object.
    unsafe fn clone_unchecked(&self) -> Self;
    /// Checks wether this resource and the other are from the same client
    ///
    /// Returns `true` if both are alive and belong to the same client, `false`
    /// otherwise.
    fn same_client_as<R: Resource>(&self, other: &R) -> bool {
        // comparing client pointers for equality is only meaningfull
        // if resources are alive
        if !(self.status() == Liveness::Alive && other.status() == Liveness::Alive) {
            false
        } else {
            let my_client =
                unsafe { ffi_dispatch!(WAYLAND_SERVER_HANDLE, wl_resource_get_client, self.ptr()) };
            let other_client =
                unsafe { ffi_dispatch!(WAYLAND_SERVER_HANDLE, wl_resource_get_client, other.ptr()) };
            my_client == other_client
        }
    }
}

/// Common trait for wayland objects that can be registered to an EventQueue
pub unsafe trait Implementable<ID: 'static>: Resource {
    /// The type containing the implementation for the event callbacks
    type Implementation: PartialEq + Copy + 'static;
    #[doc(hidden)]
    unsafe fn __dispatch_msg(&self, client: &Client, opcode: u32, args: *const wl_argument)
                             -> Result<(), ()>;
}

/// Possible outcome of the call of a event on a resource
#[derive(Debug)]
pub enum EventResult<T> {
    /// Message has been buffered and will be sent to client
    Sent(T),
    /// This resource is already destroyed, request has been ignored
    Destroyed,
}

impl<T> EventResult<T> {
    /// Assert that result is successfull and extract the value.
    ///
    /// Panics with provided error message if the result was `Destroyed`.
    pub fn expect(self, error: &str) -> T {
        match self {
            EventResult::Sent(v) => v,
            EventResult::Destroyed => panic!("{}", error),
        }
    }
}

/// Represents the state of liveness of a wayland object
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// This object is alive and events can be sent to it
    Alive,
    /// This object is dead, sending it events will do nothing and
    /// return and error.
    Dead,
    /// This object is not managed by `wayland-server`, you can send it events
    /// but this might crash the program if it was actually dead.
    Unmanaged,
}

mod generated {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(non_upper_case_globals, non_snake_case, unused_imports)]
    #![allow(missing_docs)]

    pub mod interfaces {
        //! Interfaces for the core protocol
        // You might need them for the bindings generated for protocol extensions
        include!(concat!(env!("OUT_DIR"), "/wayland_interfaces.rs"));
    }

    pub mod server {
        //! The wayland core protocol
        // This module contains all objects of the core wayland protocol.
        //
        // It has been generated from the `wayland.xml` protocol file
        // using `wayland_scanner`.
        // Imports that need to be available to submodules
        // but should not be in public API.
        // Will be fixable with pub(restricted).

        #[doc(hidden)]
        pub use super::interfaces;
        #[doc(hidden)]
        pub use {Client, EventLoopHandle, EventResult, Implementable, Liveness, Resource};

        include!(concat!(env!("OUT_DIR"), "/wayland_api.rs"));
    }
}

pub mod sys {
    //! Reexports of types and objects from wayland-sys

    pub use wayland_sys::common::*;
    pub use wayland_sys::server::*;
}
