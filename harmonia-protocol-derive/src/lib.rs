//! # Using derive
//!
//! 1. [Overview](#overview)
//! 3. [Attributes](#attributes)
//!     1. [Container attributes](#container-attributes)
//!         1. [`#[nix(from_str)]`](#nixfrom_str)
//!         2. [`#[nix(from_store_dir_str)]`](#nixfrom_store_dir_str)
//!         3. [`#[nix(from = "FromType")]`](#nixfrom--fromtype)
//!         4. [`#[nix(try_from = "FromType")]`](#nixtry_from--fromtype)
//!         5. [`#[nix(into = "IntoType")]`](#nixinto--intotype)
//!         6. [`#[nix(try_into = "IntoType")]`](#nixtry_into--intotype)
//!         7. [`#[nix(display)]`](#nixdisplay)
//!         8. [`#[nix(display = "path")]`](#nixdisplay--path)
//!         9. [`#[nix(store_dir_display)]`](#nixstore_dir_display)
//!         10. [`#[nix(crate = "...")]`](#nixcrate--)
//!     2. [Variant attributes](#variant-attributes)
//!         1. [`#[nix(version = "range")]`](#nixversion--range)
//!     3. [Field attributes](#field-attributes)
//!         1. [`#[nix(version = "range")]`](#nixversion--range-1)
//!         2. [`#[nix(default)]`](#nixdefault)
//!         3. [`#[nix(default = "path")]`](#nixdefault--path)
//!
//! ## Overview
//!
//! This crate contains derive macros and function-like macros for implementing
//! `NixDeserialize` with less boilerplate.
//!
//! ### Examples
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! struct Unnamed(u64, String);
//! ```
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! struct Fields {
//!     number: u64,
//!     message: String,
//! };
//! ```
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! struct Ignored;
//! ```
//!
//! ## Attributes
//!
//! To customize the derived trait implementations you can add
//! [attributes](https://doc.rust-lang.org/reference/attributes.html)
//! to containers, fields and variants.
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! #[nix(crate="nixrs")] // <-- This is a container attribute
//! struct Fields {
//!     number: u64,
//!     #[nix(version="..20")] // <-- This is a field attribute
//!     message: String,
//! };
//!
//! #[derive(NixDeserialize)]
//! #[nix(crate="nixrs")] // <-- This is also a container attribute
//! enum E {
//!     #[nix(version="..10")] // <-- This is a variant attribute
//!     A(u64),
//!     #[nix(version="10..")] // <-- This is also a variant attribute
//!     B(String),
//! }
//! ```
//!
//! ### Container attributes
//!
//! ##### `#[nix(from_str)]`
//!
//! When `from_str` is specified the fields are all ignored and instead a
//! `String` is first deserialized and then `FromStr::from_str` is used
//! to convert this `String` to the container type.
//!
//! This means that the container must implement `FromStr` and the error
//! returned from the `from_str` must implement `Display`.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! #[nix(from_str)]
//! struct MyString(String);
//! impl std::str::FromStr for MyString {
//!     type Err = String;
//!     fn from_str(s: &str) -> Result<Self, Self::Err> {
//!         if s != "bad string" {
//!             Ok(MyString(s.to_string()))
//!         } else {
//!             Err("Got a bad string".to_string())
//!         }
//!     }
//! }
//! ```
//!
//! ##### `#[nix(from_store_dir_str)]`
//!
//! When `from_store_dir_str` is specified the fields are all
//! ignored and instead a `String` is first deserialized and then
//! `FromStoreDirStr::from_store_dir_str` is used to convert this
//! `String` to the container type.
//!
//! This means that the container must implement `FromStoreDirStr`.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! # use nixrs::store_path::StoreDir;
//! #
//! #[derive(NixDeserialize)]
//! #[nix(from_store_dir_str)]
//! struct MyString(String);
//! impl nixrs::store_path::FromStoreDirStr for MyString {
//!     type Error = std::io::Error;
//!     fn from_store_dir_str(store_dir: &StoreDir, s: &str) -> Result<Self, Self::Error> {
//!         if s != "bad string" {
//!             Ok(MyString(s.to_string()))
//!         } else {
//!             Err(std::io::Error::new(std::io::ErrorKind::Other, "Got a bad string"))
//!         }
//!     }
//! }
//! ```
//!
//! ##### `#[nix(from = "FromType")]`
//!
//! When `from` is specified the fields are all ignored and instead a
//! value of `FromType` is first deserialized and then `From::from` is
//! used to convert from this value to the container type.
//!
//! This means that the container must implement `From<FromType>` and
//! `FromType` must implement `NixDeserialize`.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! #[nix(from="usize")]
//! struct MyValue(usize);
//! impl From<usize> for MyValue {
//!     fn from(val: usize) -> Self {
//!         MyValue(val)
//!     }
//! }
//! ```
//!
//! ##### `#[nix(try_from = "FromType")]`
//!
//! With `try_from` a value of `FromType` is first deserialized and then
//! `TryFrom::try_from` is used to convert from this value to the container
//! type.
//!
//! This means that the container must implement `TryFrom<FromType>` and
//! `FromType` must implement `NixDeserialize`.
//! The error returned from `try_from` also needs to implement `Display`.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! #[nix(try_from="usize")]
//! struct WrongAnswer(usize);
//! impl TryFrom<usize> for WrongAnswer {
//!     type Error = String;
//!     fn try_from(val: usize) -> Result<Self, Self::Error> {
//!         if val != 42 {
//!             Ok(WrongAnswer(val))
//!         } else {
//!             Err("Got the answer to life the universe and everything".to_string())
//!         }
//!     }
//! }
//! ```
//!
//! ##### `#[nix(into = "IntoType")]`
//!
//! When `into` is specified the fields are all ignored and instead the
//! container type is converted to `IntoType` using `Into::into` and
//! `IntoType` is then serialized. Before converting `Clone::clone` is
//! called.
//!
//! This means that the container must implement `Into<IntoType>` and `Clone`
//! and `IntoType` must implement `NixSerialize`.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixSerialize;
//! #
//! #[derive(Clone, NixSerialize)]
//! #[nix(into="usize")]
//! struct MyValue(usize);
//! impl From<MyValue> for usize {
//!     fn from(val: MyValue) -> Self {
//!         val.0
//!     }
//! }
//! ```
//!
//! ##### `#[nix(try_into = "IntoType")]`
//!
//! When `try_into` is specified the fields are all ignored and instead the
//! container type is converted to `IntoType` using `TryInto::try_into` and
//! `IntoType` is then serialized. Before converting `Clone::clone` is
//! called.
//!
//! This means that the container must implement `TryInto<IntoType>` and
//! `Clone` and `IntoType` must implement `NixSerialize`.
//! The error returned from `try_into` also needs to implement `Display`.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixSerialize;
//! #
//! #[derive(Clone, NixSerialize)]
//! #[nix(try_into="usize")]
//! struct WrongAnswer(usize);
//! impl TryFrom<WrongAnswer> for usize {
//!     type Error = String;
//!     fn try_from(val: WrongAnswer) -> Result<Self, Self::Error> {
//!         if val.0 != 42 {
//!             Ok(val.0)
//!         } else {
//!             Err("Got the answer to life the universe and everything".to_string())
//!         }
//!     }
//! }
//! ```
//!
//! ##### `#[nix(display)]`
//!
//! When `display` is specified the fields are all ignored and instead the
//! container must implement `Display` and `NixWrite::write_display` is used to
//! write the container.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixSerialize;
//! # use std::fmt::{Display, Result, Formatter};
//! #
//! #[derive(NixSerialize)]
//! #[nix(display)]
//! struct WrongAnswer(usize);
//! impl Display for WrongAnswer {
//!     fn fmt(&self, f: &mut Formatter<'_>) -> Result {
//!         write!(f, "Wrong Answer = {}", self.0)
//!     }
//! }
//! ```
//!
//! ##### `#[nix(display = "path")]`
//!
//! When `display` is specified the fields are all ignored and instead the
//! container the specified path must point to a function that is callable as
//! `fn(&T) -> impl Display`. The result from this call is then written with
//! `NixWrite::write_display`.
//! For example `default = "my_value"` would call `my_value(&self)` and `display =
//! "AType::empty"` would call `AType::empty(&self)`.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixSerialize;
//! # use std::fmt::{Display, Result, Formatter};
//! #
//! #[derive(NixSerialize)]
//! #[nix(display = "format_it")]
//! struct WrongAnswer(usize);
//! struct WrongDisplay<'a>(&'a WrongAnswer);
//! impl<'a> Display for WrongDisplay<'a> {
//!     fn fmt(&self, f: &mut Formatter<'_>) -> Result {
//!         write!(f, "Wrong Answer = {}", self.0.0)
//!     }
//! }
//!
//! fn format_it(value: &WrongAnswer) -> impl Display + '_ {
//!     WrongDisplay(value)
//! }
//! ```
//!
//! ##### `#[nix(store_dir_display)]`
//!
//! When `store_dir_display` is specified the fields are all ignored and instead the
//! container must implement `nixrs::store_path::StoreDirDisplay` and `StoreDir::display`
//! is used to get a `Display` value from the container and that value is then written
//! with `NixWrite::write_display`.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use std::fmt::{Display, Result, Formatter};
//! # use nixrs::store_path::{StoreDir, StoreDirDisplay};
//! # use harmonia_protocol_derive::NixSerialize;
//! #
//! #[derive(NixSerialize)]
//! #[nix(store_dir_display)]
//! struct WrongAnswer(usize);
//! impl StoreDirDisplay for WrongAnswer {
//!     fn fmt(&self, _store_dir: &StoreDir, f: &mut Formatter<'_>) -> Result {
//!         write!(f, "Wrong Answer = {}", self.0)
//!     }
//! }
//! ```
//!
//! ##### `#[nix(crate = "...")]`
//!
//! Specify the path to the `nixrs` crate instance to use when referring
//! to the Nix.rs API in the generated code. This is usually not needed.
//!
//! ### Variant attributes
//!
//! ##### `#[nix(version = "range")]`
//!
//! Specifies the protocol version range where this variant is used.
//! When deriving an enum the `version` attribute is used to select which
//! variant of the enum to deserialize. The range is for minor version and
//! the version ranges of all variants combined must cover all versions
//! without any overlap or the first variant that matches is selected.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! enum Testing {
//!     #[nix(version="..=18")]
//!     OldVersion(u64),
//!     #[nix(version="19..")]
//!     NewVersion(String),
//! }
//! ```
//!
//! ### Field attributes
//!
//! ##### `#[nix(version = "range")]`
//!
//! Specifies the protocol version range where this field is included.
//! The range is for minor version. For example `version = "..20"`
//! includes the field in protocol versions `1.0` to `1.19` and skips
//! it in version `1.20` and above.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! struct Field {
//!     number: u64,
//!     #[nix(version="..20")]
//!     messsage: String,
//! }
//! ```
//!
//! ##### `#[nix(default)]`
//!
//! When a field is skipped because the active protocol version falls
//! outside the range specified in [`#[nix(version = "range")]`](#nixversion--range-1)
//! this attribute indicates that `Default::default()` should be used
//! to get a value for the field. This is also the default
//! when you only specify [`#[nix(version = "range")]`](#nixversion--range-1).
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! struct Field {
//!     number: u64,
//!     #[nix(version="..20", default)]
//!     messsage: String,
//! }
//! ```
//!
//! ##### `#[nix(default = "path")]`
//!
//! When a field is skipped because the active protocol version falls
//! outside the range specified in [`#[nix(version = "range")]`](#nixversion--range-1)
//! this attribute indicates that the function in `path` should be called to
//! get a default value for the field. The given function must be callable
//! as `fn() -> T`.
//! For example `default = "my_value"` would call `my_value()` and `default =
//! "AType::empty"` would call `AType::empty()`.
//!
//! ###### Example
//!
//! ```rust,ignore
//! # use harmonia_protocol_derive::NixDeserialize;
//! #
//! #[derive(NixDeserialize)]
//! struct Field {
//!     number: u64,
//!     #[nix(version="..20", default="missing_string")]
//!     messsage: String,
//! }
//!
//! fn missing_string() -> String {
//!     "missing string".to_string()
//! }
//! ```

use internal::inputs::RemoteInput;
use proc_macro::TokenStream;
use syn::{DeriveInput, parse_quote};

mod de;
mod internal;
mod ser;

// Helper to extract crate path from attributes, defaulting to harmonia_protocol
fn get_crate_path(input: &DeriveInput) -> syn::Path {
    let cx = internal::Context::new();
    // Parse attributes to check for #[nix(crate = "...")]
    let attrs = internal::attrs::Container::from_ast(&cx, &input.attrs);

    // Check for errors before proceeding (ignore errors in crate path extraction)
    let _ = cx.check();

    if let Some(crate_path) = attrs.crate_path {
        return crate_path;
    }

    // Default to crate for same-crate usage (standard practice)
    // Types in other crates should use #[nix(crate = "harmonia_protocol")]
    parse_quote!(crate)
}

#[proc_macro_derive(NixDeserialize, attributes(nix))]
pub fn derive_nix_deserialize(item: TokenStream) -> TokenStream {
    let mut input = syn::parse_macro_input!(item as DeriveInput);
    let crate_path = get_crate_path(&input);
    de::expand_nix_deserialize(crate_path, &mut input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(NixSerialize, attributes(nix))]
pub fn derive_nix_serialize(item: TokenStream) -> TokenStream {
    let mut input = syn::parse_macro_input!(item as DeriveInput);
    let crate_path = get_crate_path(&input);
    ser::expand_nix_serialize(crate_path, &mut input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Macro to implement `NixDeserialize` on a type.
/// Sometimes you can't use the deriver to implement `NixDeserialize`
/// (like when dealing with types in Rust standard library) but don't want
/// to implement it yourself. So this macro can be used for those situations
/// where you would derive using `#[nix(from_str)]`,
/// `#[nix(from = "FromType")]` or `#[nix(try_from = "FromType")]` if you
/// could.
///
/// #### Example
///
/// ```rust,ignore
/// # use harmonia_protocol_derive::nix_deserialize_remote;
/// #
/// struct MyU64(u64);
///
/// impl From<u64> for MyU64 {
///     fn from(value: u64) -> Self {
///         Self(value)
///     }
/// }
///
/// nix_deserialize_remote!(#[nix(from="u64")] MyU64);
/// ```
#[proc_macro]
pub fn nix_deserialize_remote(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as RemoteInput);
    let crate_path = parse_quote!(crate);
    de::expand_nix_deserialize_remote(crate_path, &input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Macro to implement `NixSerialize` on a type.
/// Sometimes you can't use the deriver to implement `NixSerialize`
/// (like when dealing with types in Rust standard library) but don't want
/// to implement it yourself. So this macro can be used for those situations
/// where you would derive using `#[nix(display)]`, `#[nix(display = "path")]`,
/// `#[nix(store_dir_display)]`, `#[nix(into = "IntoType")]` or
/// `#[nix(try_into = "IntoType")]` if you could.
///
/// #### Example
///
/// ```rust,ignore
/// # use harmonia_protocol_derive::nix_serialize_remote;
/// #
/// #[derive(Clone)]
/// struct MyU64(u64);
///
/// impl From<MyU64> for u64 {
///     fn from(value: MyU64) -> Self {
///         value.0
///     }
/// }
///
/// nix_serialize_remote!(#[nix(into="u64")] MyU64);
/// ```
#[proc_macro]
pub fn nix_serialize_remote(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as RemoteInput);
    let crate_path = parse_quote!(crate);
    ser::expand_nix_serialize_remote(crate_path, &input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
