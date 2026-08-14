// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod fonts;
mod markdown;
pub(crate) mod writer;

pub(crate) fn render(markdown_source: &str, facet: &str, date_label: &str) -> Vec<u8> {
    let mut blocks = vec![
        markdown::Block::text(
            "FACET NEWSLETTER".to_owned(),
            fonts::Face::TimesBold,
            9.6,
            0.0,
        ),
        markdown::Block::text(
            format!("{facet} · {date_label}"),
            fonts::Face::TimesBold,
            22.8,
            0.0,
        ),
    ];
    blocks.extend(markdown::layout(markdown_source));
    writer::render(&blocks)
}
