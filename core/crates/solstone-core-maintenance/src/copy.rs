// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cli_copy::{ENGLISH, Value, display_for, render_for};
use std::fmt::Display;

#[derive(Clone, Copy)]
pub(crate) struct Output<'a>(Option<&'a str>);

impl<'a> Output<'a> {
    pub(crate) const fn terminal(locale: Option<&'a str>) -> Self {
        Self(locale)
    }

    pub(crate) const fn canonical() -> Self {
        Self(None)
    }

    pub(crate) fn locale(self) -> &'a str {
        self.0.unwrap_or(ENGLISH)
    }

    pub(crate) fn message<const N: usize>(
        self,
        id: &str,
        args: [(&str, &dyn Display); N],
    ) -> String {
        display_for(self.locale(), id, args)
    }

    pub(crate) fn line<const N: usize>(self, id: &str, args: [(&str, &dyn Display); N]) -> String {
        let mut line = self.message(id, args);
        line.push('\n');
        line
    }

    pub(crate) fn counted<const C: usize, const N: usize>(
        self,
        id: &str,
        counts: [(&str, u64); C],
        args: [(&str, &dyn Display); N],
    ) -> String {
        let values = args.map(|(name, value)| (name, value.to_string()));
        let mut fields = Vec::with_capacity(C + N);
        for (name, count) in counts {
            // Embedded and supervised receipts retain the original other-branch spelling.
            let quantity = self
                .0
                .map_or(0, |_| i64::try_from(count).unwrap_or(i64::MAX));
            fields.push((name, Value::Cardinal(quantity)));
        }
        fields.extend(
            values
                .iter()
                .map(|(name, value)| (*name, Value::String(value))),
        );
        render_for(self.locale(), id, &fields)
    }
}
