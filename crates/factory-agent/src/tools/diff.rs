use hashbrown::HashMap;

const CONTEXT: usize = 4;

enum Block<'a> {
    Equal(&'a [&'a str]),
    Removed(&'a [&'a str]),
    Added(&'a [&'a str]),
}

#[derive(Clone, Copy)]
struct Comp {
    count: usize,
    added: bool,
    removed: bool,
    prev: Option<usize>,
}

#[derive(Clone, Copy)]
struct Path {
    old_pos: i64,
    last: Option<usize>,
}

/// jsdiff's line tokens: each line keeps its `\n` or `\r\n`; a lone `\r` stays inside the line.
fn tokenize(value: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in value.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(&value[start..=index]);
            start = index + 1;
        }
    }
    if start < value.len() {
        lines.push(&value[start..]);
    }
    lines
}

fn add_to_path(comps: &mut Vec<Comp>, path: Path, added: bool, removed: bool, old_inc: i64) -> Path {
    let comp = match path.last.map(|index| comps[index]) {
        Some(last) if last.added == added && last.removed == removed => Comp { count: last.count + 1, added, removed, prev: last.prev },
        _ => Comp { count: 1, added, removed, prev: path.last },
    };
    comps.push(comp);
    Path { old_pos: path.old_pos + old_inc, last: Some(comps.len() - 1) }
}

fn extract_common(comps: &mut Vec<Comp>, base: &mut Path, old: &[&str], new: &[&str], diagonal: i64) -> i64 {
    let (old_len, new_len) = (old.len() as i64, new.len() as i64);
    let mut old_pos = base.old_pos;
    let mut new_pos = old_pos - diagonal;
    let mut common = 0;
    while new_pos + 1 < new_len && old_pos + 1 < old_len && old[(old_pos + 1) as usize] == new[(new_pos + 1) as usize] {
        new_pos += 1;
        old_pos += 1;
        common += 1;
    }
    if common > 0 {
        comps.push(Comp { count: common, added: false, removed: false, prev: base.last });
        base.last = Some(comps.len() - 1);
    }
    base.old_pos = old_pos;
    new_pos
}

fn build<'a>(comps: &[Comp], last: Option<usize>, old: &'a [&'a str], new: &'a [&'a str]) -> Vec<Block<'a>> {
    let mut chain = Vec::new();
    let mut current = last;
    while let Some(index) = current {
        chain.push(comps[index]);
        current = comps[index].prev;
    }
    chain.reverse();
    let (mut old_pos, mut new_pos) = (0, 0);
    chain
        .into_iter()
        .map(|comp| {
            if comp.removed {
                old_pos += comp.count;
                Block::Removed(&old[old_pos - comp.count..old_pos])
            } else {
                new_pos += comp.count;
                if comp.added {
                    Block::Added(&new[new_pos - comp.count..new_pos])
                } else {
                    old_pos += comp.count;
                    Block::Equal(&new[new_pos - comp.count..new_pos])
                }
            }
        })
        .collect()
}

/// jsdiff's Myers walk, removals before insertions on a tie, so the model reads the same diff.
fn diff_lines<'a>(old: &'a [&'a str], new: &'a [&'a str]) -> Vec<Block<'a>> {
    let (old_len, new_len) = (old.len() as i64, new.len() as i64);
    let mut comps: Vec<Comp> = Vec::new();
    let mut start = Path { old_pos: -1, last: None };
    let mut new_pos = extract_common(&mut comps, &mut start, old, new, 0);
    if start.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return build(&comps, start.last, old, new);
    }
    let mut best: HashMap<i64, Path> = HashMap::new();
    best.insert(0, start);
    let (mut min_diagonal, mut max_diagonal) = (i64::MIN, i64::MAX);
    let max_edit_length = old_len + new_len;
    let mut edit_length = 1;
    while edit_length <= max_edit_length {
        let mut diagonal = min_diagonal.max(-edit_length);
        let upper = max_diagonal.min(edit_length);
        while diagonal <= upper {
            let remove_path = best.remove(&(diagonal - 1));
            let add_path = best.get(&(diagonal + 1)).copied();
            let can_add = add_path.filter(|path| (0..new_len).contains(&(path.old_pos - diagonal)));
            let can_remove = remove_path.filter(|path| path.old_pos + 1 < old_len);
            let (from, added) = match (can_add, can_remove) {
                (None, None) => {
                    best.remove(&diagonal);
                    diagonal += 2;
                    continue;
                }
                (Some(add), None) => (add, true),
                (None, Some(remove)) => (remove, false),
                (Some(add), Some(remove)) => {
                    if remove.old_pos < add.old_pos {
                        (add, true)
                    } else {
                        (remove, false)
                    }
                }
            };
            let mut base = add_to_path(&mut comps, from, added, !added, i64::from(!added));
            new_pos = extract_common(&mut comps, &mut base, old, new, diagonal);
            if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return build(&comps, base.last, old, new);
            }
            if base.old_pos + 1 >= old_len {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            best.insert(diagonal, base);
            diagonal += 2;
        }
        edit_length += 1;
    }
    Vec::new()
}

struct Hunk {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
    lines: Vec<String>,
}

fn context<'a>(lines: &'a [&'a str]) -> impl Iterator<Item = String> + 'a {
    lines.iter().map(|line| format!(" {line}"))
}

/// jsdiff's `createTwoFilesPatch` with its defaults, since the model reads this text.
pub fn two_files_patch(old_name: &str, new_name: &str, old: &str, new: &str) -> String {
    let (old_tokens, new_tokens) = (tokenize(old), tokenize(new));
    let mut blocks = diff_lines(&old_tokens, &new_tokens);
    blocks.push(Block::Equal(&[]));
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut old_start, mut new_start, mut old_line, mut new_line) = (0usize, 0usize, 1usize, 1usize);
    let mut range: Vec<String> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        match block {
            Block::Removed(lines) | Block::Added(lines) => {
                if old_start == 0 {
                    old_start = old_line;
                    new_start = new_line;
                    if let Some(Block::Equal(previous)) = i.checked_sub(1).and_then(|j| blocks.get(j)) {
                        let keep = &previous[previous.len().saturating_sub(CONTEXT)..];
                        old_start -= keep.len();
                        new_start -= keep.len();
                        range.extend(context(keep));
                    }
                }
                let (prefix, added) = if matches!(block, Block::Added(_)) { ("+", true) } else { ("-", false) };
                range.extend(lines.iter().map(|line| format!("{prefix}{line}")));
                if added {
                    new_line += lines.len();
                } else {
                    old_line += lines.len();
                }
            }
            Block::Equal(lines) => {
                if old_start != 0 {
                    if lines.len() <= CONTEXT * 2 && i < blocks.len() - 2 {
                        range.extend(context(lines));
                    } else {
                        let keep = lines.len().min(CONTEXT);
                        range.extend(context(&lines[..keep]));
                        hunks.push(Hunk {
                            old_start,
                            old_lines: old_line - old_start + keep,
                            new_start,
                            new_lines: new_line - new_start + keep,
                            lines: std::mem::take(&mut range),
                        });
                        old_start = 0;
                        new_start = 0;
                    }
                }
                old_line += lines.len();
                new_line += lines.len();
            }
        }
    }
    let mut out = vec![format!("Index: {old_name}"), "=".repeat(67), format!("--- {old_name}"), format!("+++ {new_name}")];
    for hunk in hunks {
        let old_start = if hunk.old_lines == 0 { hunk.old_start - 1 } else { hunk.old_start };
        let new_start = if hunk.new_lines == 0 { hunk.new_start - 1 } else { hunk.new_start };
        out.push(format!("@@ -{old_start},{} +{new_start},{} @@", hunk.old_lines, hunk.new_lines));
        for line in hunk.lines {
            match line.strip_suffix('\n') {
                Some(stripped) => out.push(stripped.to_owned()),
                None => {
                    out.push(line);
                    out.push("\\ No newline at end of file".to_owned());
                }
            }
        }
    }
    out.join("\n") + "\n"
}
