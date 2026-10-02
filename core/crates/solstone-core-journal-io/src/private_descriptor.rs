// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure descriptor and access-mask policy for Windows private retained handles.
//!
//! This module contains no platform-specific types or FFI bindings and can be
//! tested routinely on any host.

#![allow(dead_code)]

pub(crate) const DIRECTORY_ACCESS_MASK: u32 = 0x001200A7;
// Standard owner read/write opens include EA and attribute rights. Omitting
// them would make a generic-access denial a misleading ACL boundary witness.
pub(crate) const FILE_ACCESS_MASK: u32 = 0x0013019F;
pub(crate) const LOCK_ACCESS_MASK: u32 = 0x0012019F;

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
    process_owner_sid: &[u8],
) -> Result<(), &'static str> {
    if descriptor.owner_sid.is_empty() {
        return Err("owner SID is empty");
    }
    for privileged in &PRIVILEGED_SIDS {
        if descriptor.owner_sid.as_slice() == *privileged {
            return Err("privileged owner SID is forbidden");
        }
    }
    if descriptor.owner_sid != process_owner_sid {
        return Err("descriptor owner does not match process owner");
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

/// Decode only the self-relative owner and ordinary allow-ACE layout we admit.
/// Every offset and SID length is checked before indexing; other ACE layouts
/// never enter the allow-ACE SID decoder.
pub(crate) fn parse_private_descriptor(buffer: &[u8]) -> Result<ParsedDescriptor, &'static str> {
    fn range(bytes: &[u8], start: usize, length: usize) -> Result<&[u8], &'static str> {
        bytes
            .get(
                start
                    ..start
                        .checked_add(length)
                        .ok_or("descriptor range overflow")?,
            )
            .ok_or("truncated descriptor component")
    }
    fn word(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
        Ok(u16::from_le_bytes(
            range(bytes, offset, 2)?.try_into().unwrap(),
        ))
    }
    fn dword(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(
            range(bytes, offset, 4)?.try_into().unwrap(),
        ))
    }
    fn sid(bytes: &[u8], offset: usize) -> Result<Vec<u8>, &'static str> {
        let prefix = range(bytes, offset, 8)?;
        if prefix[0] != 1 || prefix[1] > 15 {
            return Err("unsupported SID layout");
        }
        Ok(range(bytes, offset, 8 + usize::from(prefix[1]) * 4)?.to_vec())
    }
    let header = range(buffer, 0, 20)?;
    let control = word(header, 2)?;
    if header[0] != 1 || header[1] != 0 || control & SE_SELF_RELATIVE == 0 {
        return Err("unsupported security descriptor layout");
    }
    let owner_offset = dword(header, 4)? as usize;
    if owner_offset < 20 {
        return Err("missing or overlapping descriptor owner");
    }
    let owner_sid = sid(buffer, owner_offset)?;
    let dacl_offset = dword(header, 16)? as usize;
    let dacl = if control & SE_DACL_PRESENT == 0 {
        DaclState::Absent
    } else if dacl_offset == 0 {
        DaclState::Null
    } else {
        if dacl_offset < 20 {
            return Err("overlapping DACL header");
        }
        let acl_header = range(buffer, dacl_offset, 8)?;
        if acl_header[0] != 2 || word(acl_header, 4)? != 1 {
            return Err("unsupported ACL layout");
        }
        let acl = range(buffer, dacl_offset, usize::from(word(acl_header, 2)?))?;
        let ace_header = range(acl, 8, 8)?;
        if ace_header[0] != ACCESS_ALLOWED_ACE_TYPE {
            return Err("unsupported ACE layout");
        }
        let ace_size = usize::from(word(ace_header, 2)?);
        let ace = range(acl, 8, ace_size)?;
        let ace_sid = sid(ace, 8)?;
        if ace_size != 8 + ace_sid.len() || acl.len() != 8 + ace_size {
            return Err("unsupported ACE size or ACL trailing data");
        }
        DaclState::Present(vec![ParsedAce {
            ace_type: ace_header[0],
            ace_flags: ace_header[1],
            mask: dword(ace_header, 4)?,
            sid: ace_sid,
        }])
    };
    Ok(ParsedDescriptor {
        owner_sid,
        owner_defaulted: control & 0x0001 != 0,
        dacl,
        dacl_defaulted: control & 0x0008 != 0,
        dacl_auto_inherited: control & SE_DACL_AUTO_INHERITED != 0,
        dacl_protected: control & SE_DACL_PROTECTED != 0,
        self_relative: true,
        raw_control: control,
    })
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
        assert!(admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()).is_ok());

        let dir_descriptor = make_valid_descriptor(DIRECTORY_ACCESS_MASK);
        assert!(
            admit_private_descriptor(&dir_descriptor, DIRECTORY_ACCESS_MASK, &valid_user_sid())
                .is_ok()
        );

        let lock_descriptor = make_valid_descriptor(LOCK_ACCESS_MASK);
        assert!(
            admit_private_descriptor(&lock_descriptor, LOCK_ACCESS_MASK, &valid_user_sid()).is_ok()
        );
    }

    #[test]
    fn private_descriptor_other_owner_and_matching_ace_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.owner_sid[12] ^= 1;
        if let DaclState::Present(aces) = &mut descriptor.dacl {
            aces[0].sid = descriptor.owner_sid.clone();
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
            Err("descriptor owner does not match process owner")
        );
    }

    fn relative_fixture() -> Vec<u8> {
        let sid = valid_user_sid();
        let mut bytes = vec![0; 20];
        bytes[0] = 1;
        bytes[2..4].copy_from_slice(
            &(SE_DACL_PRESENT | SE_DACL_PROTECTED | SE_SELF_RELATIVE).to_le_bytes(),
        );
        bytes[4..8].copy_from_slice(&20u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&(20u32 + sid.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&sid);
        bytes.extend_from_slice(&[2, 0]);
        bytes.extend_from_slice(&(16u16 + sid.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&[1, 0, 0, 0]);
        bytes.extend_from_slice(&[ACCESS_ALLOWED_ACE_TYPE, 0]);
        bytes.extend_from_slice(&(8u16 + sid.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&FILE_ACCESS_MASK.to_le_bytes());
        bytes.extend_from_slice(&sid);
        bytes
    }

    #[test]
    fn self_relative_descriptor_decodes_and_bounds_every_prefix() {
        let bytes = relative_fixture();
        let descriptor = parse_private_descriptor(&bytes).unwrap();
        assert!(admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()).is_ok());
        for length in 0..bytes.len() {
            assert!(
                parse_private_descriptor(&bytes[..length]).is_err(),
                "prefix {length}"
            );
        }
        let mut invalid_offset = bytes.clone();
        invalid_offset[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_private_descriptor(&invalid_offset).is_err());
        let mut overlap = bytes.clone();
        overlap[4..8].copy_from_slice(&4u32.to_le_bytes());
        assert!(parse_private_descriptor(&overlap).is_err());
    }

    #[test]
    fn self_relative_descriptor_refuses_other_ace_layout_before_sid_decode() {
        let mut bytes = relative_fixture();
        let ace_offset = 20 + valid_user_sid().len() + 8;
        bytes[ace_offset] = 5;
        bytes[ace_offset + 9] = 255;
        assert_eq!(
            parse_private_descriptor(&bytes),
            Err("unsupported ACE layout")
        );
        bytes[ace_offset] = ACCESS_ALLOWED_ACE_TYPE;
        assert_eq!(
            parse_private_descriptor(&bytes),
            Err("unsupported SID layout")
        );
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
            Err("ACE flags must be zero")
        );
    }

    #[test]
    fn private_descriptor_null_dacl_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.dacl = DaclState::Null;
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
            Err("DACL is null")
        );
    }

    #[test]
    fn private_descriptor_absent_dacl_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        descriptor.dacl = DaclState::Absent;
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
            Err("descriptor owner does not match process owner")
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
            Err("ACE is not AccessAllowed")
        );
    }

    #[test]
    fn private_descriptor_broader_mask_refused() {
        let mut descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        if let DaclState::Present(ref mut aces) = descriptor.dacl {
            aces[0].mask = FILE_ACCESS_MASK | 0x00040000; // Explicit WRITE_DAC is extra.
        }
        assert_eq!(
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
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
            admit_private_descriptor(&descriptor, FILE_ACCESS_MASK, &valid_user_sid()),
            Err("ACE mask does not match required object mask")
        );
    }

    #[test]
    fn private_descriptor_input_unchanged_after_refusal() {
        let descriptor = make_valid_descriptor(FILE_ACCESS_MASK);
        let snapshot_owner = descriptor.owner_sid.clone();
        let snapshot_dacl = descriptor.dacl.clone();

        let result =
            admit_private_descriptor(&descriptor, DIRECTORY_ACCESS_MASK, &valid_user_sid());
        assert!(result.is_err());

        assert_eq!(descriptor.owner_sid, snapshot_owner);
        assert_eq!(descriptor.dacl, snapshot_dacl);
    }
}
