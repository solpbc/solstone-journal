// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpcodeTag {
    Equal,
    Replace,
    Delete,
    Insert,
}

#[cfg(test)]
impl OpcodeTag {
    fn as_str(self) -> &'static str {
        match self {
            Self::Equal => "equal",
            Self::Replace => "replace",
            Self::Delete => "delete",
            Self::Insert => "insert",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Opcode {
    pub tag: OpcodeTag,
    pub i1: usize,
    pub i2: usize,
    pub j1: usize,
    pub j2: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Match {
    pub a: usize,
    pub b: usize,
    pub size: usize,
}

pub(crate) struct SequenceMatcher<'a, T> {
    a: &'a [T],
    b: &'a [T],
    b2j: HashMap<&'a T, Vec<usize>>,
    matching_blocks: Option<Vec<Match>>,
    opcodes: Option<Vec<Opcode>>,
}

impl<'a, T: Eq + std::hash::Hash> SequenceMatcher<'a, T> {
    pub(crate) fn new(a: &'a [T], b: &'a [T]) -> Self {
        let mut b2j = HashMap::new();
        for (i, elt) in b.iter().enumerate() {
            b2j.entry(elt).or_insert_with(Vec::new).push(i);
        }
        Self {
            a,
            b,
            b2j,
            matching_blocks: None,
            opcodes: None,
        }
    }

    pub(crate) fn find_longest_match(
        &self,
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> Match {
        let mut besti = alo;
        let mut bestj = blo;
        let mut bestsize = 0;

        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut newj2len = HashMap::new();
            if let Some(indices) = self.b2j.get(&self.a[i]) {
                for &j in indices {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|prior| j2len.get(&prior))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }

        while besti > alo && bestj > blo && self.a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && self.a[besti + bestsize] == self.b[bestj + bestsize]
        {
            bestsize += 1;
        }

        Match {
            a: besti,
            b: bestj,
            size: bestsize,
        }
    }

    pub(crate) fn get_matching_blocks(&mut self) -> &[Match] {
        if self.matching_blocks.is_none() {
            let la = self.a.len();
            let lb = self.b.len();
            let mut queue = vec![(0, la, 0, lb)];
            let mut matching_blocks = Vec::new();

            while let Some((alo, ahi, blo, bhi)) = queue.pop() {
                let m = self.find_longest_match(alo, ahi, blo, bhi);
                if m.size > 0 {
                    matching_blocks.push(m);
                    if alo < m.a && blo < m.b {
                        queue.push((alo, m.a, blo, m.b));
                    }
                    if m.a + m.size < ahi && m.b + m.size < bhi {
                        queue.push((m.a + m.size, ahi, m.b + m.size, bhi));
                    }
                }
            }
            matching_blocks.sort();

            let mut i1 = 0;
            let mut j1 = 0;
            let mut k1 = 0;
            let mut non_adjacent = Vec::new();
            for m in matching_blocks {
                if i1 + k1 == m.a && j1 + k1 == m.b {
                    k1 += m.size;
                } else {
                    if k1 > 0 {
                        non_adjacent.push(Match {
                            a: i1,
                            b: j1,
                            size: k1,
                        });
                    }
                    i1 = m.a;
                    j1 = m.b;
                    k1 = m.size;
                }
            }
            if k1 > 0 {
                non_adjacent.push(Match {
                    a: i1,
                    b: j1,
                    size: k1,
                });
            }
            non_adjacent.push(Match {
                a: la,
                b: lb,
                size: 0,
            });

            self.matching_blocks = Some(non_adjacent);
        }
        self.matching_blocks.as_ref().unwrap()
    }

    pub(crate) fn get_opcodes(&mut self) -> &[Opcode] {
        if self.opcodes.is_none() {
            let blocks = self.get_matching_blocks().to_vec();
            let mut i = 0;
            let mut j = 0;
            let mut answer = Vec::new();

            for m in blocks {
                let (ai, bj, size) = (m.a, m.b, m.size);
                let tag = if i < ai && j < bj {
                    Some(OpcodeTag::Replace)
                } else if i < ai {
                    Some(OpcodeTag::Delete)
                } else if j < bj {
                    Some(OpcodeTag::Insert)
                } else {
                    None
                };
                if let Some(tag) = tag {
                    answer.push(Opcode {
                        tag,
                        i1: i,
                        i2: ai,
                        j1: j,
                        j2: bj,
                    });
                }
                i = ai + size;
                j = bj + size;
                if size > 0 {
                    answer.push(Opcode {
                        tag: OpcodeTag::Equal,
                        i1: ai,
                        i2: i,
                        j1: bj,
                        j2: j,
                    });
                }
            }

            self.opcodes = Some(answer);
        }
        self.opcodes.as_ref().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_docstring_example() {
        let a: Vec<char> = "qabxcd".chars().collect();
        let b: Vec<char> = "abycdf".chars().collect();
        let mut matcher = SequenceMatcher::new(&a, &b);
        let opcodes = matcher.get_opcodes();

        let tags: Vec<(&str, usize, usize, usize, usize)> = opcodes
            .iter()
            .map(|op| (op.tag.as_str(), op.i1, op.i2, op.j1, op.j2))
            .collect();

        assert_eq!(
            tags,
            vec![
                ("delete", 0, 1, 0, 0),
                ("equal", 1, 3, 0, 2),
                ("replace", 3, 4, 2, 3),
                ("equal", 4, 6, 3, 5),
                ("insert", 6, 6, 5, 6),
            ]
        );
    }
}
