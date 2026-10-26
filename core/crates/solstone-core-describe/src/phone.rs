// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A screen recorded on the owner's own phone or tablet.
//!
//! The frame prompts were written for a desktop. A phone frame is one app in
//! portrait, and two things on it mislead a model that thinks it is reading a
//! desktop: the phone's own recording panel ("Screen Sharing", "Stop
//! Sharing") reads as the owner sharing to a call, and a picture-in-picture
//! call window reads as the call showing whatever app is underneath. The
//! segment's capture metadata says which device recorded it, so phone frames
//! get a short note; a desktop frame's prompt is unchanged.

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhoneScreen {
    device: &'static str,
}

impl PhoneScreen {
    /// The phone or tablet a segment's capture metadata names, if any.
    pub fn from_segment_meta(meta: &Value) -> Option<Self> {
        let device = match meta.get("platform").and_then(Value::as_str)? {
            "ios" => "iPhone or iPad",
            "android" => "Android phone or tablet",
            _ => return None,
        };
        Some(Self { device })
    }

    /// The capture metadata the sense dispatcher hands a segment's describe run.
    pub fn from_environment() -> Option<Self> {
        let meta = std::env::var("SEGMENT_META").ok()?;
        Self::from_segment_meta(&serde_json::from_str(&meta).ok()?)
    }

    /// The opening line of a frame's categorization request.
    pub fn frame_text(&self) -> String {
        format!(
            "Analyze this screenshot frame from a screen recording the owner made on their own {}.",
            self.device
        )
    }

    /// The opening line of a frame's extraction request.
    pub fn extraction_text(&self, category: &str) -> String {
        format!(
            "Analyze this {category} screenshot from a screen recording the owner made on their own {}.",
            self.device
        )
    }

    /// Appended to the categorization instruction.
    pub fn describe_note(&self) -> String {
        format!(
            "This frame is not a desktop. It comes from a screen recording the owner made on their own {}, so it shows a phone or tablet screen, usually one app filling it.\n\
             - Describe the app on screen, not windows on a desktop.\n\
             - A \"Screen Sharing\", \"Screen Recording\" or \"Stop Sharing\" panel, or a red recording dot or pill in the status bar, is this device recording its own screen. It is not the owner sharing their screen to a call.\n\
             - A small call window floating over another app is picture-in-picture. When the app beneath it fills most of the screen, that app is the primary category and the call is secondary.",
            self.device
        )
    }

    /// Appended to the meeting extraction instruction.
    pub fn meeting_note(&self) -> String {
        format!(
            "This screenshot comes from a screen recording the owner made on their own {}.\n\
             - platform: name a platform only when its name or logo is visible in the screenshot; otherwise use \"other\".\n\
             - screen_share: the device's own recording panel is not a screen share, and neither is an app shown beneath a small picture-in-picture call window, which is what the owner is looking at on their device. Set screen_share to null unless the call's own window shows a presentation.\n\
             - A participant labelled \"You\" is the owner.",
            self.device
        )
    }
}

#[cfg(test)]
mod tests {
    use super::PhoneScreen;
    use serde_json::json;

    #[test]
    fn only_a_phone_or_tablet_platform_is_a_phone_screen() {
        let ios =
            PhoneScreen::from_segment_meta(&json!({"platform":"ios","sources":["screencast"]}));
        assert!(ios.unwrap().frame_text().contains("iPhone or iPad"));
        let android = PhoneScreen::from_segment_meta(&json!({"platform":"android"}));
        assert!(
            android
                .unwrap()
                .meeting_note()
                .contains("Android phone or tablet")
        );
        for meta in [
            json!({"platform":"macos"}),
            json!({"platform":"linux"}),
            json!({}),
        ] {
            assert_eq!(PhoneScreen::from_segment_meta(&meta), None);
        }
    }
}
