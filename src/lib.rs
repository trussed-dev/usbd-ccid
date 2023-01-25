#![no_std]

//! CCID descriptor and USB CCID class implementation.
//!
//! This crate implements CCID communication to a USB host, and sends the resulting APDUs to an [Interchange](https://docs.rs/interchange)
//!
//! [CCID Specification for Integrated Circuit(s) Cards Interface Devices](https://www.usb.org/sites/default/files/DWG_Smart-Card_CCID_Rev110.pdf)
//!
//! [CCID SpecificationUSB Integrated Circuit(s) Card Devices](https://www.usb.org/sites/default/files/DWG_Smart-Card_USB-ICC_ICCD_rev10.pdf)

#[macro_use]
extern crate delog;
generate_macros!();

pub mod class;
pub mod constants;
pub mod pipe;
pub mod types;

// pub mod piv;

pub use class::Ccid;
