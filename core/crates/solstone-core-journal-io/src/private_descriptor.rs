// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure descriptor and access-mask policy for Windows private retained handles.
//!
//! This module contains no platform-specific types or FFI bindings and can be
//! tested routinely on any host.

#![allow(dead_code)]

pub(crate) const DIRECTORY_ACCESS_MASK: u32 = 0x001200A7;
pub(crate) const FILE_ACCESS_MASK: u32 = 0x00130083;
pub(crate) const LOCK_ACCESS_MASK: u32 = 0x00120083;

pub(crate) const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

pub(crate) const SE_DACL_PRESENT: u16 = 0x0004;
#[allow(dead_code)]
pub(crate) const SE_DACL_AUTO_INHERITED: u16 = 0x0400;
pub(crate) const SE_DACL_PROTECTED: u16 = 0x1000;
pub(crate) const SE_SELF_RELATIVE: u16 = 0x8000;

/// Binary SIDs of well-known privileged/system accounts that must be refused.
pub(crate) const PRIVILEGED_SIDS: [&[u8]; 4] = [
    // S-1-5-18 (LocalSystem)
    &[
        0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x12, 0x00, 0x00, 0x00,
    ],
    // S-1-5-19 (LocalService)
    &[
        0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x13, 0x00, 0x00, 0x00,
    ],
    // S-1-5-20 (NetworkService)
    &[
        0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x14, 0x00, 0x00, 0x00,
    ],
    // S-1-5-32-544 (Builtin Administrators)
    &[
        0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x20, 0x00, 0x00, 0x00, 0x20, 0x02, 0x00,
        0x00,
    ],
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParsedAce {
    pub ace_type: u8,
    pub ace_flags: u8,
    pub mask: u32,
    pub sid: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DaclState {
    Absent,
    Null,
    Present(Vec<ParsedAce>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParsedDescriptor {
    pub owner_sid: Vec<u8>,
    pub owner_defaulted: bool,
    pub dacl: DaclState,
    pub dacl_defaulted: bool,
    pub dacl_auto_inherited: bool,
    pub dacl_protected: bool,
    pub self_relative: bool,
    pub raw_control: u16,
}

/// Pure validation of a queried security descriptor against required owner-only policy.
pub(crate) fn admit_private_descriptor(
    descriptor: &ParsedDescriptor,
    needed_mask: u32,
) -> Result<(), &'static str> {
    if descriptor.owner_sid.is_empty() {
        return Err("owner SID is empty");
    }
    for privileged in &PRIVILEGED_SIDS {
        if descriptor.owner_sid.as_slice() == *privileged {
            return Err("privileged owner SID is forbidden");
        }
    }
    if descriptor.owner_defaulted {
        return Err("owner is defaulted");
    }
    if descriptor.dacl_defaulted {
        return Err("DACL is defaulted");
    }
    if descriptor.dacl_auto_inherited {
        return Err("DACL is auto-inherited");
    }
    if !descriptor.dacl_protected {
        return Err("DACL is not protected");
    }

    let allowed_control_mask = SE_DACL_PRESENT | SE_DACL_PROTECTED | SE_SELF_RELATIVE;
    if (descriptor.raw_control & !allowed_control_mask) != 0 {
        return Err("unsupported control bits present");
    }

    let aces = match &descriptor.dacl {
        DaclState::Present(aces) => aces,
        DaclState::Null => return Err("DACL is null"),
        DaclState::Absent => return Err("DACL is absent"),
    };

    if aces.len() != 1 {
        return Err("DACL must have exactly one ACE");
    }

    let ace = &aces[0];
    if ace.ace_type != ACCESS_ALLOWED_ACE_TYPE {
        return Err("ACE is not AccessAllowed");
    }
    if ace.ace_flags != 0 {
        return Err("ACE flags must be zero");
    }
    if ace.mask != needed_mask {
        return Err("ACE mask does not match required object mask");
    }
    if ace.sid != descriptor.owner_sid {
        return Err("ACE SID does not match owner SID");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_user_sid() -> Vec<u8> {
        // S-1-5-21-1111-2222-3333-1001
        vec![
            0x01, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x15, 0x00, 0x00, 0x00, 0x57, 0x04,
            0x00, 0x00, 0xAE, 0x08, 0x00, 0x00, 0x05, 0x0D, 0x00, 0x00, 0xE9, 0x03, 0x00, 0x00,
        ]
    }

    fn make_valid_descriptor(mask: u32) -> ParsedDescriptor {
        let sid = valid_user_sid();
        ParsedDescriptor {
            owner_sid: sid.clone(),
            owner_defaulted: false,
            dacl: DaclState::Present(vec![ParsedAce {
                ace_type: ACCESS_ALLOWED_ACE_TYPE,
                ace_flags: 0,
                mask,
                sid,
            }]),
            dacl_defaulted: false,
            dacl_auto_inherited: false,
            dacl_protected: true,
            self_relative: true,
            raw_control: SE_DACL_PRESENT | SE_DACL_PROTECTED | SE_SELF_RELATIVE,
        }
    }

    #[test]
    fn private_descriptor_owner_only_pass() {
        let descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        assert!(admit_private_descriptor(&descriptor, FILE_ACCESS_MASK).is_ok());

        let dir_descriptor = make_valid_descriptor(DIRECTORY_ACCESS_MASK);
        assert!(admit_private_descriptor(&dir_descriptor, DIRECTORY_ACCESS_MASK).is_ok());

        let lock_descriptor = make_valid_descriptor(LOCK_ACCESS_MASK);
        assert!(admit_private_descriptor(&lock_descriptor, LOCK_ACCESS_MASK).is_ok());
    }

    #[test]
    fn private_descriptor_other_sid_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        let other_sid = vec![
            0x01, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x15, 0x00, 0x00, 0x00, 0x99, 0x04,
            0x00, 0x00, 0xAE, 0x08, 0x00, 0x00, 0x05, 0x0D, 0x00, 0x00, 0xEA, 0x03, 0x00, 0x00,
        ];
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].sid = other_sid;
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("ACE SID does not match owner SID")
        );
    }

    #[test]
    fn private_descriptor_non_zero_flags_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].ace_flags = 0x01; // OBJECT_INHERIT_ACE
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("ACE flags must be zero")
        );
    }

    #[test]
    fn private_descriptor_null_dacl_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.dacl = DaclState::Null;
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("DACL is null")
        );
    }

    #[test]
    fn private_descriptor_absent_dacl_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.dacl = DaclState::Absent;
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("DACL is absent")
        );
    }

    #[test]
    fn private_descriptor_wrong_owner_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.owner_sid = vec![
            0x01, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x15, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        ];
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("ACE SID does not match owner SID")
        );
    }

    #[test]
    fn private_descriptor_privileged_owner_s1_5_18_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.owner_sid = PRIVILEGED_SIDS[0].to_vec();
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].sid = descriptor.owner_sid.clone();
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("privileged owner SID is forbidden")
        );
    }

    #[test]
    fn private_descriptor_privileged_owner_s1_5_19_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.owner_sid = PRIVILEGED_SIDS[1].to_vec();
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].sid = descriptor.owner_sid.clone();
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("privileged owner SID is forbidden")
        );
    }

    #[test]
    fn private_descriptor_privileged_owner_s1_5_20_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.owner_sid = PRIVILEGED_SIDS[2].to_vec();
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].sid = descriptor.owner_sid.clone();
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("privileged owner SID is forbidden")
        );
    }

    #[test]
    fn private_descriptor_privileged_owner_s1_5_32_544_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.owner_sid = PRIVILEGED_SIDS[3].to_vec();
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].sid = descriptor.owner_sid.clone();
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("privileged owner SID is forbidden")
        );
    }

    #[test]
    fn private_descriptor_object_ace_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].ace_type = 5; // ACCESS_ALLOWED_OBJECT_ACE_TYPE
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("ACE is not AccessAllowed")
        );
    }

    #[test]
    fn private_descriptor_unknown_ace_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].ace_type = 1; // ACCESS_DENIED_ACE_TYPE
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("ACE is not AccessAllowed")
        );
    }

    #[test]
    fn private_descriptor_broader_mask_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].mask = FILE_ACCESS_MASK | 0x00000004; // Extra right
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("ACE mask does not match required object mask")
        );
    }

    #[test]
    fn private_descriptor_narrower_mask_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].mask = FILE_ACCESS_MASK & !0x00000001; // Missing right
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK),
            Err("ACE mask does not match required object mask")
        );
    }

    #[test]
    fn private_descriptor_input_unchanged_after_refusal() {
        let descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        let snapshot_owner = descriptor.owner_sid.clone();
        let snapshot_dacl = descriptor.dacl.clone();

        let result = admit_private_descriptor(&descriptor, DIRECTORY_ACCESS_MASK);
        assert!(result.is_err());

        assert_eq!(descriptor.owner_sid, snapshot_owner);
        assert_eq!(descriptor.dacl, snapshot_dacl);
    }
}
