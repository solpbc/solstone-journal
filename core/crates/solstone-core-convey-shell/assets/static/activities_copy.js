// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Exposes activity participation copy as window.solActivitiesCopy.
// Keys: sectionAttendees, sectionMentioned, provVoice, provSpeakerLabel,
// provTranscript, provScreen, provOther, lessCertain, empty, unavailable.
(function () {
  window.solActivitiesCopy = {
    sectionAttendees: "Attendees",
    sectionMentioned: "Mentioned",
    provVoice: "heard them speak in this meeting",
    provSpeakerLabel: "named in the meeting panel",
    provTranscript: "named in transcript only",
    provScreen: "appeared on screen",
    provOther: "noted in this activity",
    lessCertain: "less certain",
    empty: "We didn't find anyone in this activity.",
    unavailable: "We couldn't read this activity's people."
  };
})();
