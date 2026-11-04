// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Current observed artifact regions, carried by the existing exact packets.
//! Coordinates are transcript-relative; no live artifact is opened during folding.

use super::*;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

const REGION_OPEN: &str = "[ARTIFACT ";
const REGION_CLOSE: &str = "[/ARTIFACT]";

static NUMBERED: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^\+?(\d+):(?:[[:xdigit:]]+\|| ?)(.*)$").expect("numbered observation"));
static DELETED: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^-(\d+):[[:xdigit:]]+(?:\.\.(\d+):[[:xdigit:]]+)?").expect("deleted region"));
static SHIFT: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"after (\d+):[[:xdigit:]]+ ([+-]\d+)").expect("coordinate shift"));

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegionHeader {
	path: String,
	start: usize,
	lines: usize,
}

#[derive(Debug, Clone)]
struct Observation {
	path: String,
	start: usize,
	lines: Vec<String>,
}

#[derive(Debug, Clone)]
struct KnownLine {
	text: String,
	packet: usize,
	source_line: usize,
	observed: usize,
}

type Artifacts = BTreeMap<String, BTreeMap<usize, KnownLine>>;

fn args(call: &Value) -> Value {
	let value = call.get("function").unwrap_or(call);
	let value = value
		.get("arguments")
		.or_else(|| value.get("args"))
		.unwrap_or(&Value::Null);
	match value {
		Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
		_ => value.clone(),
	}
}

fn calls(messages: &[Message]) -> BTreeMap<String, (String, Value)> {
	messages
		.iter()
		.filter_map(|m| m.tool_calls.as_ref()?.as_array())
		.flatten()
		.filter_map(|call| {
			let id = call.get("id")?.as_str()?.to_string();
			let name = call.get("function").unwrap_or(call).get("name")?.as_str()?.to_string();
			Some((id, (name, args(call))))
		})
		.collect()
}

fn paths(args: &Value) -> Vec<String> {
	let mut found = crate::supervisor::detect::param_paths(args);
	for key in ["edits", "operations"] {
		if let Some(items) = args.get(key).and_then(Value::as_array) {
			for item in items {
				found.extend(paths(item));
			}
		}
	}
	found
		.iter_mut()
		.for_each(|p| *p = p.trim_start_matches("./").replace('\\', "/"));
	found.sort();
	found.dedup();
	found
}

fn path_header(line: &str, paths: &[String]) -> Option<String> {
	let line = line.strip_prefix("FILE: ").unwrap_or(line).trim();
	// An absolute spelling from the same result can resolve to a relative
	// argument only when that suffix identifies exactly one requested artifact.
	let matches: Vec<_> = paths
		.iter()
		.filter(|p| line == p.as_str() || line.ends_with(&format!("/{p}")))
		.collect();
	(matches.len() == 1).then(|| matches[0].clone())
}

fn sections<'a>(content: &'a str, paths: &[String]) -> Vec<(String, Vec<&'a str>)> {
	let mut out: Vec<(String, Vec<&str>)> = Vec::new();
	let mut path = (paths.len() == 1).then(|| paths[0].clone());
	for line in content.lines() {
		if let Some(header) = path_header(line, paths) {
			path = Some(header);
		}
		if let Some(path) = &path {
			if out.last().is_none_or(|(p, _)| p != path) {
				out.push((path.clone(), Vec::new()));
			}
			out.last_mut().expect("section inserted").1.push(line);
		}
	}
	out
}

fn changes<'a>(args: &'a Value, path: &str) -> Vec<&'a Value> {
	if let Some(edits) = args.get("edits").and_then(Value::as_array) {
		return edits.iter().filter(|e| paths(e).iter().any(|p| p == path)).collect();
	}
	if let Some(operations) = args.get("operations").and_then(Value::as_array) {
		return operations.iter().collect();
	}
	vec![args]
}

fn observations(content: &str, args: &Value) -> Vec<Observation> {
	let paths = paths(args);
	let mut out = Vec::new();
	for (path, lines) in sections(content, &paths) {
		let mut numbered = BTreeMap::new();
		let mut added = BTreeMap::new();
		for line in &lines {
			if let Some(c) = NUMBERED.captures(line) {
				let n: usize = c[1].parse().expect("digits");
				if n == 0 {
					continue;
				}
				// Hash ranges and omission markers are not observed bytes.
				if !line.contains('|') {
					continue;
				}
				numbered.insert(n, c[2].to_string());
				if line.starts_with('+') {
					added.insert(n, c[2].to_string());
				}
			}
		}
		// Some successful editors elide the middle as "lines as sent". The
		// acknowledged first/last lines locate the exact submitted replacement;
		// unacknowledged arguments alone must never become observed content.
		for change in changes(args, &path) {
			let replacement = change
				.get("new_text")
				.or_else(|| change.get("content"))
				.and_then(Value::as_str);
			if let Some(text) = replacement {
				let replacement: Vec<_> = text.lines().collect();
				let starts: Vec<_> = added
					.iter()
					.filter(|(_, text)| replacement.first() == Some(&text.as_str()))
					.filter(|(n, _)| {
						replacement
							.last()
							.is_some_and(|last| added.get(&(*n + replacement.len() - 1)).is_some_and(|v| v == last))
					})
					.map(|(n, _)| *n)
					.collect();
				if starts.len() == 1 {
					for (offset, line) in replacement.iter().enumerate() {
						numbered.insert(starts[0] + offset, (*line).to_string());
					}
				}
			}
		}
		for (n, text) in numbered {
			if let Some(last) = out
				.last_mut()
				.filter(|r: &&mut Observation| r.path == path && r.start + r.lines.len() == n)
			{
				last.lines.push(text);
			} else {
				out.push(Observation {
					path: path.clone(),
					start: n,
					lines: vec![text],
				});
			}
		}
	}
	out
}

fn render_region(path: &str, start: usize, lines: &[String]) -> String {
	let header = RegionHeader {
		path: path.to_string(),
		start,
		lines: lines.len(),
	};
	format!(
		"{REGION_OPEN}{}]\n{}\n{REGION_CLOSE}",
		serde_json::to_string(&header).expect("region header"),
		lines.join("\n")
	)
}

/// Canonical packet rendering is also used by archive validation. Expanding
/// acknowledged replacements here keeps their spans reconstructible from the
/// archived call/result pair, without reading a potentially changed artifact.
pub(super) fn snapshot(content: &str) -> String {
	region_lines(content)
		.into_iter()
		.map(|(h, _, lines)| render_region(&h.path, h.start, &lines))
		.collect::<Vec<_>>()
		.join("\n")
}

pub(super) fn has_regions(messages: &[Message], packet: &EvidencePacket) -> bool {
	!region_lines(&render_packet(messages, packet, usize::MAX)).is_empty()
}

pub(super) fn canonical_result(message: &Message, packet_messages: &[Message]) -> String {
	let calls = calls(packet_messages);
	let Some((_, args)) = message.tool_call_id.as_ref().and_then(|id| calls.get(id)) else {
		return message.content.trim().to_string();
	};
	let regions = observations(&message.content, args);
	if regions.is_empty() {
		return message.content.trim().to_string();
	}
	regions
		.iter()
		.map(|r| render_region(&r.path, r.start, &r.lines))
		.collect::<Vec<_>>()
		.join("\n")
}

fn region_lines(content: &str) -> Vec<(RegionHeader, usize, Vec<String>)> {
	let lines: Vec<_> = content.lines().collect();
	let mut result = Vec::new();
	let mut i = 0;
	while i < lines.len() {
		if let Some(header) = lines[i]
			.strip_prefix(REGION_OPEN)
			.and_then(|s| s.strip_suffix(']'))
			.and_then(|s| serde_json::from_str::<RegionHeader>(s).ok())
		{
			let end = i.saturating_add(header.lines).saturating_add(1);
			if header.start > 0 && end < lines.len() && lines[end] == REGION_CLOSE {
				result.push((
					header,
					i + 2,
					lines[i + 1..end].iter().map(|s| (*s).to_string()).collect(),
				));
				i = end;
			}
		}
		i += 1;
	}
	result
}

fn invalidate(artifacts: &mut Artifacts, name: &str, args: &Value, content: &str) {
	if !crate::supervisor::detect::is_mutation_call(name, args)
		&& args.get("new_text").is_none()
		&& args.get("edits").is_none()
		&& args.get("operations").is_none()
	{
		return;
	}
	let paths = paths(args);
	if paths.is_empty() {
		// A mutation with no bounded subjects cannot prove any stored version
		// current. Conservative eviction is preferable to replaying stale bytes.
		artifacts.clear();
		return;
	}
	let sections = sections(content, &paths);
	for path in paths {
		let Some(old) = artifacts.remove(&path) else {
			continue;
		};
		let Some((_, lines)) = sections.iter().find(|(p, _)| p == &path) else {
			continue;
		};
		let deleted: Vec<(usize, usize)> = lines
			.iter()
			.filter_map(|s| DELETED.captures(s))
			.map(|c| {
				let start = c[1].parse().expect("digits");
				(
					start,
					c.get(2).map_or(start, |end| end.as_str().parse().expect("digits")),
				)
			})
			.collect();
		let shifts: BTreeMap<usize, isize> = lines
			.iter()
			.filter(|s| s.starts_with("shift:"))
			.flat_map(|s| SHIFT.captures_iter(s))
			.map(|c| (c[1].parse().expect("digits"), c[2].parse().expect("signed digits")))
			.collect();
		// A locationless acknowledgement cannot establish the unaffected range.
		if deleted.is_empty() && shifts.is_empty() {
			continue;
		}
		let mut current = BTreeMap::new();
		for (n, line) in old {
			if deleted.iter().any(|(start, end)| *start <= n && n <= *end) {
				continue;
			}
			let delta = shifts.range(..n).next_back().map_or(0, |(_, delta)| *delta);
			if let Some(n) = n.checked_add_signed(delta).filter(|n| *n > 0) {
				current.insert(n, line);
			}
		}
		artifacts.insert(path, current);
	}
}

/// Build a latest-version union before ranking. Even observations that will
/// lose the budget competition participate in invalidation and deduplication.
fn current_lines(packets: &[EvidencePacket], messages: &[Message]) -> Artifacts {
	let mut artifacts = Artifacts::new();
	for (index, packet) in packets.iter().enumerate() {
		let canonical = render_packet(messages, packet, usize::MAX);
		let observe = |content: &str, source_offset: usize, artifacts: &mut Artifacts| {
			for (header, source, lines) in region_lines(content) {
				for (offset, text) in lines.into_iter().enumerate() {
					artifacts.entry(header.path.clone()).or_default().insert(
						header.start + offset,
						KnownLine {
							text,
							packet: index,
							source_line: source_offset + source + offset,
							observed: packet.message_end,
						},
					);
				}
			}
		};
		if packet.kind == PacketKind::ArtifactSnapshot {
			observe(&canonical, 0, &mut artifacts);
			continue;
		}
		let slice = &messages[packet.message_start..=packet.message_end];
		let calls = calls(slice);
		// Result order matters inside a batch too: a later mutation must not
		// resurrect an earlier read just because they share an atomic packet.
		for (offset, message) in slice.iter().enumerate().filter(|(_, m)| m.role == "tool") {
			if let Some((name, args)) = message.tool_call_id.as_ref().and_then(|id| calls.get(id)) {
				invalidate(&mut artifacts, name, args, &message.content);
			}
			let marker = format!("[MESSAGE {} TOOL RESULT", packet.message_start + offset + 1);
			if let Some(line) = canonical.lines().position(|line| line.starts_with(&marker)) {
				observe(&canonical_result(message, slice), line + 1, &mut artifacts);
			}
		}
	}
	artifacts
}

/// Every admitted region stays in its source packet's KeepExact lane. Regions
/// can span several old reads, but their union has only one value per line.
pub(super) fn select(packets: &mut [EvidencePacket], messages: &[Message], budget: usize, focus: &str) -> usize {
	let artifacts = current_lines(packets, messages);
	let mut regions: Vec<(String, usize, Vec<KnownLine>, usize, f64)> = Vec::new();
	for (path, lines) in artifacts {
		let references: Vec<_> = messages
			.iter()
			.enumerate()
			.filter(|(_, m)| {
				m.content.contains(&path) || m.tool_calls.as_ref().is_some_and(|c| c.to_string().contains(&path))
			})
			.map(|(i, _)| i)
			.collect();
		let last_reference = references.last().copied().unwrap_or_default();
		// Repeated use and recency express utility; divide by rendered cost
		// below so one large observation cannot crowd out the working set.
		let utility = references.len() as f64 + last_reference as f64 / messages.len().max(1) as f64;

		let focused = usize::from(focus.contains(&path));
		for (n, line) in lines {
			if let Some((_, _, held, _, _)) = regions.last_mut().filter(|(p, start, held, _, _)| {
				*p == path
					&& *start + held.len() == n
					&& held
						.last()
						.is_some_and(|prev| prev.packet == line.packet && prev.source_line + 1 == line.source_line)
			}) {
				held.push(line);
			} else {
				regions.push((path.clone(), n, vec![line], focused, utility));
			}
		}
	}
	let score = |region: &(String, usize, Vec<KnownLine>, usize, f64)| {
		let text: Vec<_> = region.2.iter().map(|line| line.text.clone()).collect();
		let cost = crate::session::estimate_tokens(&render_region(&region.0, region.1, &text));
		(region.4 + region.2[0].observed as f64 / messages.len().max(1) as f64) / cost.max(1) as f64
	};
	regions.sort_by(|a, b| {
		b.3.cmp(&a.3)
			.then_with(|| score(b).total_cmp(&score(a)))
			.then_with(|| b.2[0].observed.cmp(&a.2[0].observed))
			.then_with(|| a.0.cmp(&b.0))
			.then(a.1.cmp(&b.1))
	});

	let canonical: Vec<_> = packets.iter().map(|p| render_packet(messages, p, usize::MAX)).collect();
	let mut used = 0;
	for (path, start, lines, _, _) in regions {
		let index = lines[0].packet;
		let text: Vec<_> = lines.iter().map(|line| line.text.clone()).collect();
		let region = render_region(&path, start, &text);
		let header_cost = if packets[index].lane == Lane::KeepExact {
			0
		} else {
			let mut packet = packets[index].clone();
			packet.lane = Lane::KeepExact;
			crate::session::estimate_tokens(&packet_header(&packet))
		};
		let cost = crate::session::estimate_tokens(&region) + header_cost;
		if cost > budget.saturating_sub(used) {
			continue;
		}
		let source: Vec<_> = canonical[index].lines().collect();
		let packet = &mut packets[index];
		packet.lane = Lane::KeepExact;
		// A located observation needs its source span, not an old runtime
		// steering event. Task governance remains independently pinned.
		packet.depends_on.clear();
		packet.prompt_content.push_str(&region);
		packet.prompt_content.push('\n');
		packet.exact_spans.push(source_span(
			&source,
			lines[0].source_line,
			lines.last().expect("nonempty region").source_line,
		));
		used += cost;
	}
	used
}

#[cfg(test)]
#[path = "working_set_tests.rs"]
mod tests;

/// Descriptors are navigation, so their subjects must stay cheaper than recall.
pub(super) fn describe(messages: &[Message]) -> String {
	const DESCRIPTOR_TOKENS: usize = 64;
	let mut subjects = BTreeSet::new();
	for (name, args) in calls(messages).values() {
		let paths = paths(args);
		if paths.is_empty() {
			subjects.insert(name.clone());
		} else {
			subjects.extend(paths);
		}
	}
	crate::session::truncate_to_tokens(&subjects.into_iter().collect::<Vec<_>>().join(", "), DESCRIPTOR_TOKENS)
}

pub(super) fn unretained_ranges(
	packets: &[EvidencePacket],
	path: &str,
	start: usize,
	end: usize,
) -> Vec<(usize, usize)> {
	let held: BTreeSet<usize> = packets
		.iter()
		.filter(|p| p.lane == Lane::KeepExact)
		.flat_map(|p| region_lines(&p.prompt_content))
		.filter(|(h, _, _)| h.path == path.trim_start_matches("./"))
		.flat_map(|(h, _, _)| h.start..h.start + h.lines)
		.collect();
	let mut ranges: Vec<(usize, usize)> = Vec::new();
	for n in start..=end {
		if held.contains(&n) {
			continue;
		}
		if let Some((_, last)) = ranges.last_mut().filter(|(_, last)| *last + 1 == n) {
			*last = n;
		} else {
			ranges.push((n, n));
		}
	}
	ranges
}

/// Background folds must recheck intervening mutations before committing.
/// Admit no new material here: the compression model never saw that suffix.
pub(super) fn revalidate(packets: &mut [EvidencePacket], source: &[Message], tail: &[Message]) {
	if tail.is_empty() {
		return;
	}
	let mut artifacts = current_lines(packets, source);
	for (path, lines) in &mut artifacts {
		lines.retain(|n, line| {
			packets[line.packet].lane == Lane::KeepExact
				&& region_lines(&packets[line.packet].prompt_content)
					.iter()
					.any(|(h, _, _)| h.path == *path && h.start <= *n && *n < h.start + h.lines)
		});
	}
	let calls = calls(tail);
	for message in tail.iter().filter(|m| m.role == "tool") {
		if let Some((name, args)) = message.tool_call_id.as_ref().and_then(|id| calls.get(id)) {
			invalidate(&mut artifacts, name, args, &message.content);
		}
	}
	let canonical: Vec<_> = packets.iter().map(|p| render_packet(source, p, usize::MAX)).collect();
	for packet in packets
		.iter_mut()
		.filter(|p| p.lane == Lane::KeepExact && !region_lines(&p.prompt_content).is_empty())
	{
		packet.prompt_content.clear();
		packet.exact_spans.clear();
		packet.lane = Lane::ArchiveReference;
	}
	for (path, lines) in artifacts {
		let mut groups: Vec<(usize, Vec<KnownLine>)> = Vec::new();
		for (n, line) in lines {
			if let Some((_, group)) = groups.last_mut().filter(|(start, group)| {
				*start + group.len() == n
					&& group
						.last()
						.is_some_and(|p| p.packet == line.packet && p.source_line + 1 == line.source_line)
			}) {
				group.push(line);
			} else {
				groups.push((n, vec![line]));
			}
		}
		for (start, lines) in groups {
			let packet = &mut packets[lines[0].packet];
			packet.lane = Lane::KeepExact;
			packet.prompt_content.push_str(&render_region(
				&path,
				start,
				&lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
			));
			packet.prompt_content.push('\n');
			let original: Vec<_> = canonical[lines[0].packet].lines().collect();
			packet.exact_spans.push(source_span(
				&original,
				lines[0].source_line,
				lines.last().expect("region").source_line,
			));
		}
	}
}
