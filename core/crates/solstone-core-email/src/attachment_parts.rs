// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Metadata admission for the email source. These are candidates. The shared
//! importer checks the decoded content before exposing an item in the journal.

use std::collections::HashMap;

use crate::html;
use crate::model::{Header, Part};

/// Transfer-encoded octets per automatically selected part.
pub const AUTO_ENCODED_LIMIT: usize = 8 * 1024 * 1024;
/// Absolute transfer and decoded ceilings, including owner-requested retries.
pub const HARD_LIMIT: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentKind {
    Pdf,
    Document,
    Text,
    Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateDisposition {
    Eligible,
    InlineUncertain,
    InlineDecorative,
    Unsupported,
    TooLarge,
}

#[derive(Clone, Debug)]
pub struct AttachmentCandidate<'a> {
    pub part: &'a Part,
    pub name: String,
    pub inline: bool,
    pub disposition: CandidateDisposition,
    pub kind: Option<AttachmentKind>,
}

fn supported_kind(part: &Part) -> Option<AttachmentKind> {
    use solstone_core_format::attachment::{FileType, file_type};
    Some(
        match file_type(part.filename.as_deref().unwrap_or(""), &part.mime)? {
            FileType::Pdf => AttachmentKind::Pdf,
            FileType::Document => AttachmentKind::Document,
            FileType::Text => AttachmentKind::Text,
            FileType::Jpeg | FileType::Png | FileType::WebP | FileType::Gif => {
                AttachmentKind::Image
            }
        },
    )
}

fn content_id(part: &Part) -> Option<&str> {
    part.content_id
        .as_deref()
        .map(|s| s.trim().trim_matches(['<', '>']))
        .filter(|s| !s.is_empty())
}

fn discover<'a>(part: &'a Part, related_resource: bool, out: &mut Vec<(&'a Part, bool)>) {
    if part.mime.is_empty()
        && !part.is_attachment()
        && content_id(part).is_none()
        && part.children.is_empty()
    {
        return;
    }
    if part.is_multipart() && !part.is_attachment() && part.mime != "multipart/encrypted" {
        let root = if part.mime == "multipart/related" {
            Some(
                part.start
                    .as_deref()
                    .and_then(|s| {
                        part.children
                            .iter()
                            .position(|p| content_id(p) == Some(s.trim_matches(['<', '>'])))
                    })
                    .unwrap_or(0),
            )
        } else {
            None
        };
        for (i, child) in part.children.iter().enumerate() {
            discover(
                child,
                related_resource || root.is_some_and(|root| root != i),
                out,
            );
        }
        return;
    }

    if part.mime.is_empty() {
        return;
    }

    // MIME body representations aren't attachments. An explicitly attached
    // text or HTML file is still an occurrence and keeps its name.
    if !part.is_attachment() && matches!(part.mime.as_str(), "text/plain" | "text/html") {
        return;
    }
    let explicit_file = part.disposition.as_deref() == Some("attachment");
    let inline = related_resource
        || (!explicit_file
            && (part.disposition.as_deref() == Some("inline") || content_id(part).is_some()));
    out.push((part, inline));
}

/// Discover leaf occurrences in MIME order. Without HTML this is a header-only
/// preview: an inline candidate cannot be automatically admitted until its
/// actual role is known. Never descend into attached mail or encrypted bundles.
#[must_use]
pub fn candidates<'a>(header: &'a Header, html: Option<&str>) -> Vec<AttachmentCandidate<'a>> {
    let mut parts = Vec::new();
    discover(&header.tree, false, &mut parts);
    let mut ids = HashMap::new();
    for (part, _) in &parts {
        if let Some(id) = content_id(part) {
            *ids.entry(id).or_insert(0usize) += 1;
        }
    }
    let regions = html
        .filter(|_| parts.iter().any(|(_, inline)| *inline))
        .map(html::image_regions)
        .unwrap_or_default();
    parts
        .into_iter()
        .map(|(part, inline)| {
            let kind = supported_kind(part);
            let unique_cid = content_id(part).filter(|id| ids.get(*id) == Some(&1));
            let role = if inline && kind == Some(AttachmentKind::Image) {
                match unique_cid {
                    Some(id) if regions.authored.contains(id) => CandidateDisposition::Eligible,
                    Some(id) if regions.omitted.contains(id) && !regions.uncertain.contains(id) => {
                        CandidateDisposition::InlineDecorative
                    }
                    _ => CandidateDisposition::InlineUncertain,
                }
            } else if inline {
                CandidateDisposition::InlineUncertain
            } else {
                CandidateDisposition::Eligible
            };
            let disposition = if kind.is_none() || part.path.is_empty() || part.size == 0 {
                CandidateDisposition::Unsupported
            } else if role != CandidateDisposition::Eligible {
                role
            } else if part.size > AUTO_ENCODED_LIMIT as u64 {
                CandidateDisposition::TooLarge
            } else {
                role
            };
            let name = part
                .filename
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| part.mime.clone());
            AttachmentCandidate {
                part,
                name,
                inline,
                disposition,
                kind,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Addr;
    use crate::select::{RULES_VERSION, STALE_SKIP_BEFORE_VERSION, evaluate_decisions};

    fn part(index: u32, mime: &str, name: Option<&str>) -> Part {
        Part {
            path: vec![index],
            mime: mime.into(),
            filename: name.map(str::to_owned),
            size: 80,
            ..Part::default()
        }
    }

    #[test]
    fn authored_images_and_separate_files_are_admitted_without_remote_reads() {
        let inline = |index: u32, id: &str| Part {
            disposition: Some("inline".into()),
            content_id: Some(format!("<{id}>")),
            ..part(index, "image/png", Some("image001.png"))
        };
        let header = Header {
            tree: Part {
                mime: "multipart/mixed".into(),
                children: vec![
                    part(1, "text/html", None),
                    inline(2, "body@id"),
                    inline(3, "signature"),
                    inline(4, "quote"),
                    inline(5, "unknown"),
                    part(6, "application/pdf", None),
                    part(7, "application/octet-stream", Some("receipt.pdf")),
                    part(8, "application/zip", Some("archive.pdf")),
                    Part {
                        size: AUTO_ENCODED_LIMIT as u64 + 1,
                        ..part(9, "application/pdf", Some("large.pdf"))
                    },
                    part(10, "text/plain", Some("log.txt")),
                ],
                ..Part::default()
            },
            ..Header::default()
        };
        let markup = r#"<img src="cid:body%40id"><img src="https://example.invalid/unknown">
            <div class="gmail_signature"><img src="cid:signature"><img src="cid:body%40id"></div>
            <blockquote type="cite"><img src="cid:quote"></blockquote>"#;
        let found = candidates(&header, Some(markup));
        let dispositions: Vec<_> = found.iter().map(|c| c.disposition).collect();
        use CandidateDisposition::{
            Eligible, InlineDecorative, InlineUncertain, TooLarge, Unsupported,
        };
        assert_eq!(
            dispositions,
            [
                Eligible,
                InlineDecorative,
                InlineUncertain,
                InlineUncertain,
                Eligible,
                Eligible,
                Unsupported,
                TooLarge,
                Eligible
            ]
        );
        let confirmed = candidates(
            &header,
            Some(
                r#"<blockquote type="cite"><div class="gmail_attr">On Friday Lee wrote:</div><img src="cid:quote"></blockquote>"#,
            ),
        );
        assert_eq!(confirmed[2].disposition, InlineDecorative);
        assert_eq!(found[4].name, "application/pdf");
        assert_eq!(candidates(&header, None)[0].disposition, InlineUncertain);
    }

    #[test]
    fn ambiguous_cid_and_related_image_without_inline_disposition_require_a_choice() {
        let header = Header {
            tree: Part {
                mime: "multipart/related".into(),
                children: vec![
                    part(1, "text/html", None),
                    Part {
                        content_id: Some("<shared>".into()),
                        ..part(2, "image/png", Some("1.png"))
                    },
                    Part {
                        content_id: Some("<shared>".into()),
                        ..part(3, "image/png", Some("2.png"))
                    },
                    part(4, "image/png", Some("3.png")),
                ],
                ..Part::default()
            },
            ..Header::default()
        };
        let found = candidates(&header, Some("<img src='cid:shared'>"));
        assert_eq!(found.len(), 3);
        assert!(
            found
                .iter()
                .all(|c| c.disposition == CandidateDisposition::InlineUncertain)
        );
    }

    #[test]
    fn selection_preserves_a_note_with_schedule_and_pdf_and_still_excludes_scheduling_mail() {
        let owner = "owner@example.invalid";
        let calendar = part(2, "text/calendar", Some("meeting.ics"));
        let mut h = Header {
            key: "gm:123".into(),
            labels: vec!["\\Sent".into()],
            from: vec![Addr {
                name: None,
                address: Some(owner.into()),
            }],
            tree: Part {
                mime: "multipart/mixed".into(),
                children: vec![
                    part(1, "text/plain", None),
                    calendar.clone(),
                    part(3, "application/pdf", Some("agenda.pdf")),
                ],
                ..Part::default()
            },
            ..Header::default()
        };
        let select = |hdr: Header| {
            evaluate_decisions(
                owner,
                &[owner.into()],
                &[hdr],
                &Default::default(),
                &Default::default(),
                RULES_VERSION,
                STALE_SKIP_BEFORE_VERSION,
            )
        };
        assert_eq!(select(h.clone())[0].outcome, "selected");
        assert_eq!(
            candidates(&h, None)[0].disposition,
            CandidateDisposition::Unsupported
        );
        h.tree = calendar;
        assert_eq!(select(h)[0].reason.as_deref(), Some("calendar_part"));
    }

    #[test]
    fn pure_rust_document_attachments_are_admitted_while_body_html_and_spoofed_names_stay_rejected()
    {
        let explicit_html = Part {
            disposition: Some("attachment".into()),
            ..part(6, "text/html", Some("clip.html"))
        };
        let header = Header {
            tree: Part {
                mime: "multipart/mixed".into(),
                children: vec![
                    // MIME body HTML is still skipped and never appears as a candidate.
                    part(1, "text/html", None),
                    part(
                        2,
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                        Some("proposal.docx"),
                    ),
                    part(3, "application/epub+zip", Some("book.epub")),
                    part(
                        4,
                        "application/vnd.oasis.opendocument.text",
                        Some("draft.odt"),
                    ),
                    part(5, "application/rtf", Some("memo.rtf")),
                    explicit_html,
                    part(7, "text/csv", Some("metrics.csv")),
                    part(8, "message/rfc822", None),
                    part(
                        9,
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                        None,
                    ),
                    // Spoofed/mismatched extensions stay Unsupported.
                    part(10, "application/zip", Some("archive.pdf")),
                    part(11, "text/html", Some("meeting.ics")),
                    part(12, "text/csv", Some("script.js")),
                ],
                ..Part::default()
            },
            ..Header::default()
        };
        let found = candidates(&header, None);
        assert_eq!(found.len(), 11);
        for candidate in &found[..8] {
            assert_eq!(
                candidate.kind,
                Some(AttachmentKind::Document),
                "{}",
                candidate.name
            );
            assert_eq!(
                candidate.disposition,
                CandidateDisposition::Eligible,
                "{}",
                candidate.name
            );
        }
        assert_eq!(found[6].name, "message/rfc822");
        assert_eq!(
            found[7].name,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        for candidate in &found[8..] {
            assert_eq!(
                candidate.disposition,
                CandidateDisposition::Unsupported,
                "{}",
                candidate.name
            );
        }
    }
}
