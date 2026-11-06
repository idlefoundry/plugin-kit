//! A pack's drawings: a strict subset of SVG, read first by an XML parser that refuses a DTD
//! (and so every entity), and checked element by element before usvg sees it.
//!
//! Allowed: shapes, paths, groups, gradients, clipping, masks, patterns, `use` of the drawing's
//! own parts, a few filters, and pictures embedded as PNG data. Refused: scripts, foreign
//! content, animation, text (a pack's legends are its layout's, printed by the plug-in), style
//! sheets, markers, event attributes, and any reference outside the drawing (a file, a URL).
//! Then its cost: elements (each `use` counted as all it repeats), path segments, filters and
//! blurs, each within [`Limits`], and, when asked, a test drawing at the largest size the
//! editor shows it, against a time limit.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

use crate::{Limits, Report};

mod memory;

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// The elements a drawing may hold.
const ALLOWED: &[&str] = &[
    "svg",
    "g",
    "defs",
    "title",
    "desc",
    "metadata",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "linearGradient",
    "radialGradient",
    "stop",
    "clipPath",
    "mask",
    "pattern",
    "symbol",
    "use",
    "image",
    "filter",
    "feGaussianBlur",
    "feOffset",
    "feFlood",
    "feComposite",
    "feMerge",
    "feMergeNode",
    "feColorMatrix",
    "feBlend",
    "feDropShadow",
];

/// Why an element is refused, for those a creator might try.
fn refused(name: &str) -> &'static str {
    match name {
        "script" | "foreignObject" | "iframe" | "handler" | "listener" => {
            "code or foreign content: a pack holds no code"
        }
        "animate" | "animateMotion" | "animateTransform" | "animateColor" | "set" => {
            "animation: a pack's drawings are still (what moves is the plug-in's)"
        }
        "text" | "tspan" | "textPath" | "tref" | "font" | "font-face" => {
            "text: a pack's legends are its layout's, printed by the plug-in in the pack's fonts"
        }
        "style" => "a style sheet: give each element its own attributes",
        "marker" => "markers: draw each mark as its own shape",
        "a" | "view" | "switch" | "cursor" => "an interactive element",
        _ => "an element a pack's drawing may not hold",
    }
}

/// Whether a reference stays inside the drawing (`#name`).
fn internal(href: &str) -> bool {
    href.starts_with('#')
}

/// Checks every `url(...)` in an attribute's value: each must be `#name`.
fn urls_internal(value: &str) -> bool {
    let mut rest = value;
    while let Some(i) = rest.find("url(") {
        let after = rest[i + 4..].trim_start();
        let after = after.trim_start_matches(['\'', '"']);
        if !after.starts_with('#') {
            return false;
        }
        rest = &rest[i + 4..];
    }
    true
}

/// The drawing's text read strictly, its elements and attributes checked: `Err` with why not.
fn scan(text: &str, limits: &Limits) -> Result<(), String> {
    depth_of(text)?;
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: u32::try_from(limits.svg_nodes * 4).unwrap_or(u32::MAX),
        },
    )
    .map_err(|e| format!("not well-formed XML without a DTD: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" || root.tag_name().namespace() != Some(SVG_NS) {
        return Err("not an SVG document (its root is not `svg` in SVG's namespace)".into());
    }
    // The ids references resolve to: SVG's elements outside `metadata`, as usvg keeps them;
    // one given twice, or one inside `metadata`, refused (either would let the count follow
    // another element than the one usvg draws).
    let mut ids: HashMap<&str, roxmltree::Node<'_, '_>> = HashMap::new();
    for node in doc.descendants() {
        if node.is_pi() {
            return Err("a processing instruction".into());
        }
        if !node.is_element() {
            continue;
        }
        // Only the plain `id`, as usvg reads it; one in another namespace (`xml:id`) refused,
        // so that the count and usvg never resolve a name to different elements.
        if node
            .attributes()
            .any(|a| a.name() == "id" && a.namespace().is_some())
        {
            return Err("an `id` in a namespace (`xml:id`): give elements a plain `id`".into());
        }
        let Some(id) = node.attribute("id") else {
            continue;
        };
        let in_metadata = node
            .ancestors()
            .skip(1)
            .any(|a| a.tag_name().name() == "metadata");
        if in_metadata || node.tag_name().namespace() != Some(SVG_NS) {
            return Err(format!(
                "`{id}`: an id on an element that is not SVG's, or inside `metadata`"
            ));
        }
        if ids.insert(id, node).is_some() {
            return Err(format!("`{id}`: an id given to two elements"));
        }
    }
    for node in doc.descendants().filter(roxmltree::Node::is_element) {
        let tag = node.tag_name();
        let name = tag.name();
        // A style sheet or a script by its name in any namespace, anywhere: usvg reads every
        // `style` element it finds, wherever it is.
        if matches!(name, "style" | "script") {
            return Err(format!("`{name}`: {}", refused(name)));
        }
        // Inside `metadata`, a licence's RDF in its own namespaces, not drawn: nothing of SVG's
        // may hide there.
        if node
            .ancestors()
            .skip(1)
            .any(|a| a.tag_name().name() == "metadata")
        {
            if tag.namespace() == Some(SVG_NS) {
                return Err(format!(
                    "`{name}` inside `metadata`: only other namespaces' data"
                ));
            }
            continue;
        }
        if tag.namespace() != Some(SVG_NS) {
            return Err(format!("`{name}`: an element outside SVG's namespace"));
        }
        if !ALLOWED.contains(&name) {
            return Err(format!("`{name}`: {}", refused(name)));
        }
        for attr in node.attributes() {
            let an = attr.name();
            let ns = attr.namespace();
            if an.to_ascii_lowercase().starts_with("on") {
                return Err(format!(
                    "`{name}` has `{an}`, an event handler: a pack holds no code"
                ));
            }
            if an == "href" && (ns.is_none() || ns == Some(XLINK_NS)) {
                let v = attr.value().trim();
                let ok = if name == "image" {
                    v.starts_with("data:image/png;base64,")
                } else {
                    internal(v)
                };
                if !ok {
                    return Err(format!(
                        "`{name}` refers to `{}`: a drawing refers only to its own parts (and \
                         its pictures are PNG data inside it)",
                        v.chars().take(60).collect::<String>()
                    ));
                }
            } else if !urls_internal(attr.value()) {
                return Err(format!(
                    "`{name}`'s `{an}` refers outside the drawing: only `url(#name)` is allowed"
                ));
            }
            if an == "style" {
                let v = attr.value().to_ascii_lowercase();
                if v.contains("@import") || v.contains("expression(") {
                    return Err(format!("`{name}`'s style imports or computes"));
                }
            }
        }
    }
    // What the references repeat: each element counted as many times as it is drawn or used.
    let inherited = inherited_patterns(root, &ids);
    let mut memo = HashMap::new();
    let (total, _) = expanded(
        root,
        &ids,
        &inherited,
        &mut memo,
        &mut Vec::new(),
        limits.svg_nodes,
    )?;
    if total > limits.svg_nodes {
        return Err(format!(
            "{total} elements drawn (each reference, a `use` or a `url(#...)`, counted as what it \
             repeats), more than {}",
            limits.svg_nodes
        ));
    }
    Ok(())
}

/// roxmltree 0.20's tokenizer is recursive too. Bound lexical nesting before it reads the
/// XML, leaving all names, attributes, namespaces and well-formedness to the strict parser.
fn depth_of(mut text: &str) -> Result<(), String> {
    let mut depth = 0usize;
    while let Some(at) = text.find('<') {
        text = &text[at..];
        let skip = if text.starts_with("<!--") {
            Some("-->")
        } else if text.starts_with("<![CDATA[") {
            Some("]]>")
        } else if text.starts_with("<?") {
            Some("?>")
        } else {
            None
        };
        if let Some(end) = skip {
            let n = text
                .find(end)
                .ok_or("an unfinished XML comment, CDATA or instruction")?;
            text = &text[n + end.len()..];
            continue;
        }
        if text.starts_with("<!") {
            return Err("a DTD or other XML declaration: only SVG without a DTD".into());
        }
        let mut quote = None;
        let mut end = None;
        for (i, b) in text.bytes().enumerate().skip(1) {
            if let Some(q) = quote {
                if b == q {
                    quote = None;
                }
            } else if b == b'\'' || b == b'"' {
                quote = Some(b);
            } else if b == b'>' {
                end = Some(i);
                break;
            }
        }
        let n = end.ok_or("an unfinished XML element")?;
        if text.starts_with("</") {
            depth = depth.saturating_sub(1);
        } else if !text[..n].ends_with('/') {
            depth += 1;
            if depth > 64 {
                return Err("elements nested more than 64 deep".into());
            }
        }
        text = &text[n + 1..];
    }
    Ok(())
}

/// The ids an element refers to: its `href` (a `use`'s part, a gradient's or a pattern's
/// template) and each `url(#name)` in its attributes (a fill, a stroke, a clip, a mask, a
/// filter, its style).
fn references<'a>(node: roxmltree::Node<'a, '_>) -> Vec<&'a str> {
    let mut out = Vec::new();
    for attr in node.attributes() {
        let v = attr.value();
        if attr.name() == "href" {
            if let Some(id) = v.trim().strip_prefix('#') {
                out.push(id);
            }
            continue;
        }
        let mut rest = v;
        while let Some(i) = rest.find("url(") {
            let after = rest[i + 4..].trim_start().trim_start_matches(['\'', '"']);
            if let Some(id) = after.strip_prefix('#') {
                let end = id
                    .find(|c: char| c == ')' || c == '\'' || c == '"' || c.is_whitespace())
                    .unwrap_or(id.len());
                out.push(&id[..end]);
            }
            rest = &rest[i + 4..];
        }
    }
    out
}

/// A `fill` or `stroke` as usvg resolves it on one element.
#[derive(Clone, Copy)]
struct Paint<'a> {
    value: &'a str,
    important: bool,
    /// An ancestor's, by `inherit`.
    inherited: bool,
}

/// One property's declarations on an element, in order: its attribute, then its style's (read
/// by usvg's own tokenizer), each with whether it is `!important`.
fn declarations<'a>(node: roxmltree::Node<'a, '_>, property: &str) -> Vec<(&'a str, bool)> {
    let mut out: Vec<_> = node
        .attribute(property)
        .map(|v| (v, false))
        .into_iter()
        .collect();
    if let Some(style) = node.attribute("style") {
        out.extend(
            simplecss::DeclarationTokenizer::from(style)
                .filter(|d| d.name == property)
                .map(|d| (d.value, d.important)),
        );
    }
    out
}

/// The declaration that wins as usvg 0.45 resolves them: each replaces the one before unless
/// that one is `!important` (so the first `!important` stays); `inherit` copies the nearest
/// ancestor's value with that ancestor's importance, and sets nothing when none has one.
fn usvg_paint<'a>(declared: &[(&'a str, bool)], above: Option<Paint<'a>>) -> Option<Paint<'a>> {
    let mut paint: Option<Paint<'a>> = None;
    for &(value, important) in declared {
        let new = if value.trim() == "inherit" {
            let Some(above) = above else { continue };
            Paint {
                inherited: true,
                ..above
            }
        } else {
            Paint {
                value,
                important,
                inherited: false,
            }
        };
        if !paint.is_some_and(|p| p.important) {
            paint = Some(new);
        }
    }
    paint
}

/// The value that wins as CSS cascades them: `!important` over the rest, then the style over the
/// attribute, then the later; `inherit` takes the parent's. With whether it is the parent's.
fn css_paint<'a>(declared: &[(&'a str, bool)], above: Option<&'a str>) -> Option<(&'a str, bool)> {
    let (_, &(value, _)) = declared
        .iter()
        .enumerate()
        .max_by_key(|&(i, &(_, important))| (important, i))?;
    if value.trim() == "inherit" {
        above.map(|v| (v, true))
    } else {
        Some((value, false))
    }
}

/// The pattern a paint's value names, if it names one.
fn pattern_named<'a>(
    value: &'a str,
    ids: &HashMap<&str, roxmltree::Node<'_, '_>>,
) -> Option<&'a str> {
    let id = value
        .trim()
        .strip_prefix("url(")
        .map(|v| v.trim_start().trim_start_matches(['\'', '"']))
        .and_then(|v| v.strip_prefix('#'))?;
    let id = &id[..id
        .find(|c: char| c == ')' || c == '\'' || c == '"' || c.is_whitespace())
        .unwrap_or(id.len())];
    ids.get(id)
        .is_some_and(|t| t.tag_name().name() == "pattern")
        .then_some(id)
}

/// For each shape or `use`, the patterns it paints with that it inherits (a `fill` or `stroke`
/// that an ancestor sets and it does not, or that it takes by `inherit`): a pattern's own
/// elements inherit from the pattern's ancestors, so a pattern inside a group filled with it
/// fills its own elements with itself, and usvg's reading recurses until its stack overflows
/// (found by the fuzzer: the root's `fill='url(#p)'`). Which of an element's declarations wins
/// is decided as usvg decides it and as CSS does, and a pattern either inherits counts (a
/// review: `fill:red; fill:inherit` is `inherit`). Each element's declarations are read once.
fn inherited_patterns<'a>(
    root: roxmltree::Node<'a, '_>,
    ids: &HashMap<&str, roxmltree::Node<'_, '_>>,
) -> HashMap<roxmltree::NodeId, Vec<&'a str>> {
    const PAINTS: &[&str] = &[
        "path", "rect", "circle", "ellipse", "line", "polyline", "polygon", "use",
    ];
    // What each element passes down: for a fill and a stroke, usvg's value and CSS's.
    type Down<'a> = [(Option<Paint<'a>>, Option<&'a str>); 2];
    let mut down: HashMap<roxmltree::NodeId, Down<'a>> = HashMap::new();
    let mut out = HashMap::new();
    // (Descendants in document order: each parent before its children.)
    for node in root.descendants().filter(roxmltree::Node::is_element) {
        let above = node
            .parent_element()
            .and_then(|p| down.get(&p.id()))
            .copied()
            .unwrap_or_default();
        let mut passed = above;
        let mut patterns = Vec::new();
        for (i, property) in ["fill", "stroke"].into_iter().enumerate() {
            let declared = declarations(node, property);
            let (usvg_above, css_above) = above[i];
            let usvg = usvg_paint(&declared, usvg_above);
            let css = css_paint(&declared, css_above);
            if PAINTS.contains(&node.tag_name().name()) {
                // An ancestor's value, where that is what it paints with.
                let usvg_inherits = match usvg {
                    Some(p) => p.inherited.then_some(p.value),
                    None => usvg_above.map(|p| p.value),
                };
                let css_inherits = match css {
                    Some((v, inherited)) => inherited.then_some(v),
                    None => css_above,
                };
                for id in [usvg_inherits, css_inherits]
                    .into_iter()
                    .flatten()
                    .filter_map(|v| pattern_named(v, ids))
                {
                    if !patterns.contains(&id) {
                        patterns.push(id);
                    }
                }
            }
            passed[i] = (usvg.or(usvg_above), css.map(|(v, _)| v).or(css_above));
        }
        down.insert(node.id(), passed);
        if !patterns.is_empty() {
            out.insert(node.id(), patterns);
        }
    }
    out
}

/// How many elements `node` draws, each of its references counted as what it repeats (`Err` on
/// a loop or past `most`).
fn expanded<'a, 'i>(
    node: roxmltree::Node<'a, 'i>,
    ids: &HashMap<&str, roxmltree::Node<'a, 'i>>,
    inherited: &HashMap<roxmltree::NodeId, Vec<&'a str>>,
    memo: &mut HashMap<roxmltree::NodeId, (usize, usize)>,
    path: &mut Vec<roxmltree::NodeId>,
    most: usize,
) -> Result<(usize, usize), String> {
    if path.len() >= 64 {
        return Err("elements and references nested more than 64 deep".into());
    }
    if let Some(&(n, depth)) = memo.get(&node.id()) {
        if path.len() + depth > 64 {
            return Err("elements and references nested more than 64 deep".into());
        }
        return Ok((n, depth));
    }
    if path.contains(&node.id()) {
        return Err(
            "a reference that repeats itself (a `use` or a `url(#...)`, or a pattern's elements \
             filled with the pattern they are in)"
                .into(),
        );
    }
    path.push(node.id());
    let mut n = 1usize;
    let mut depth = 1usize;
    for child in node.children().filter(roxmltree::Node::is_element) {
        let (count, height) = expanded(child, ids, inherited, memo, path, most)?;
        n = n.saturating_add(count);
        depth = depth.max(height + 1);
        if n > most {
            break;
        }
    }
    for id in references(node)
        .into_iter()
        .chain(inherited.get(&node.id()).into_iter().flatten().copied())
    {
        if n > most {
            break;
        }
        if let Some(target) = ids.get(id) {
            let (count, height) = expanded(*target, ids, inherited, memo, path, most)?;
            n = n.saturating_add(count);
            depth = depth.max(height + 1);
        }
    }
    path.pop();
    if path.len() + depth > 64 {
        return Err("elements and references nested more than 64 deep".into());
    }
    memo.insert(node.id(), (n, depth));
    Ok((n, depth))
}

/// usvg's options for a pack's drawing: no file is ever read for it (a reference that the scan
/// let past, which none should, resolves to nothing), no font, and pictures as PNG data only.
pub fn options() -> usvg::Options<'static> {
    usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|mime, data: Arc<Vec<u8>>, _| {
                (mime == "image/png").then_some(usvg::ImageKind::PNG(data))
            }),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    }
}

/// [`options`], with each embedded picture's header read within `limits` before usvg sees it
/// (usvg places a picture by the size its header claims, and panics on a vast one): why one is
/// refused, into `refused`.
fn options_within(
    limits: Limits,
    refused: Arc<std::sync::Mutex<Option<String>>>,
) -> usvg::Options<'static> {
    usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(move |mime, data: Arc<Vec<u8>>, _| {
                if mime != "image/png" {
                    return None;
                }
                match crate::picture::header(&data, &limits) {
                    Ok(_) => Some(usvg::ImageKind::PNG(data)),
                    Err(why) => {
                        *refused.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
                        None
                    }
                }
            }),
            resolve_string: Box::new(|_, _| None),
        },
        ..options()
    }
}

/// What a drawing's tree costs to draw.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cost {
    pub nodes: usize,
    pub segments: usize,
    pub filters: usize,
    pub blur: f32,
    /// A pattern's tiles over what it fills, all its uses together, and the elements those
    /// tiles draw (each tile draws the pattern's).
    pub tiles: f64,
    pub tile_nodes: f64,
    /// At the test drawing's size: the pixel rows the edges of every path cross (what drawing
    /// a path costs, more than its segments' count: a long line crosses many), and the pixels
    /// painted (fills, strokes, pictures, and each layer a group draws into: a filter's region
    /// once a primitive, a clip's, a mask's, a group's opacity).
    pub edges: f64,
    pub area: f64,
    /// What drawing it costs, in pixel-equivalents at the test drawing's size: each pixel row an
    /// edge crosses [`EDGE`], each pixel painted 1, each pixel of a layer [`LAYER`], of a filter's
    /// region [`FILTER`] a primitive (measured on an M4 Pro: about 1.6 ms a million).
    pub work: f64,
    /// The pixels of the pictures embedded in it, decoded (counted with the pack's own).
    pub pixels: u64,
    /// An upper bound on bytes allocated for drawing buffers, including intermediate ones.
    pub buffer_bytes: u64,
}

/// What an edge's pixel row, a layer's pixel and a filter primitive's pixel cost against a
/// pixel painted (measured, decisions.md R-SKIN-PACKS).
pub const EDGE: f64 = 100.0;
pub const LAYER: f64 = 6.0;
pub const FILTER: f64 = 10.0;

impl Cost {
    /// Why it is past `limits`, if it is.
    fn over(&self, limits: &Limits) -> Result<(), String> {
        if [
            self.tiles,
            self.tile_nodes,
            self.edges,
            self.area,
            self.work,
        ]
        .iter()
        .any(|n| !n.is_finite())
        {
            return Err("the drawing's counted cost is not finite".into());
        }
        if self.nodes > limits.svg_nodes {
            return Err(format!("more than {} elements drawn", limits.svg_nodes));
        }
        if self.segments > limits.svg_segments {
            return Err(format!("more than {} path segments", limits.svg_segments));
        }
        if self.filters > limits.svg_filters {
            return Err(format!(
                "more than {} filter primitives",
                limits.svg_filters
            ));
        }
        if self.blur > limits.svg_blur {
            return Err(format!(
                "a blur of {}, more than {}",
                self.blur, limits.svg_blur
            ));
        }
        if self.tiles > limits.svg_tiles as f64 {
            return Err(format!("a pattern's tiles, more than {}", limits.svg_tiles));
        }
        if self.tile_nodes > limits.svg_nodes as f64 {
            return Err(format!(
                "more than {} elements drawn in a pattern's tiles",
                limits.svg_nodes
            ));
        }
        if self.work > limits.svg_work {
            return Err(format!(
                "drawing it costs about {:.0} million pixel-equivalents (its edges {:.1} million \
                 pixel rows, {:.0} million pixels painted), more than {:.0} million",
                self.work / 1e6,
                self.edges / 1e6,
                self.area / 1e6,
                limits.svg_work / 1e6
            ));
        }
        Ok(())
    }
}

/// The pixel rows a path's edges cross and its length in pixels, `k` pixels a unit, its points
/// placed by `t` (a curve's by its control points' extent and polygon, which hold it).
fn edges_of(path: &resvg::tiny_skia::Path, t: Transform, k: f64) -> (f64, f64, usize) {
    use resvg::tiny_skia::{PathSegment, Point};
    let at = |mut p: Point| {
        t.map_point(&mut p);
        (f64::from(p.x), f64::from(p.y))
    };
    let (mut rows, mut length, mut count) = (0.0, 0.0, 0usize);
    let (mut last, mut start) = ((0.0, 0.0), (0.0, 0.0));
    for seg in path.segments() {
        let ps: Vec<(f64, f64)> = match seg {
            PathSegment::MoveTo(p) => {
                last = at(p);
                start = last;
                continue;
            }
            PathSegment::LineTo(p) => vec![at(p)],
            PathSegment::QuadTo(a, b) => vec![at(a), at(b)],
            PathSegment::CubicTo(a, b, c) => vec![at(a), at(b), at(c)],
            PathSegment::Close => vec![start],
        };
        let lo = ps.iter().map(|p| p.1).fold(last.1, f64::min);
        let hi = ps.iter().map(|p| p.1).fold(last.1, f64::max);
        rows += (hi - lo) * k;
        let mut from = last;
        for p in &ps {
            length += (p.0 - from.0).hypot(p.1 - from.1) * k;
            from = *p;
        }
        last = ps.last().copied().unwrap_or(last);
        count += 1;
    }
    (rows, length, count)
}

/// A rectangle's area, `k` pixels a unit.
fn area(w: f32, h: f32, k: f64) -> f64 {
    f64::from(w.max(0.0)) * f64::from(h.max(0.0)) * k * k
}

/// The elements a group draws (a pattern's tile's).
fn nodes_in(group: &usvg::Group) -> usize {
    1 + group
        .children()
        .iter()
        .map(|n| match n {
            usvg::Node::Group(g) => nodes_in(g),
            _ => 1,
        })
        .sum::<usize>()
}

/// Counts what `group` costs into `cost`, `k` pixels a unit at the test drawing's size, stopping
/// as soon as it is past `limits`. `in_pattern`: inside a pattern's tile, where another pattern
/// is refused and the parts' cost is the tiles' (counted where the pattern is used).
fn cost_of(
    group: &usvg::Group,
    cost: &mut Cost,
    pictures: &mut Vec<Arc<Vec<u8>>>,
    limits: &Limits,
    (k, in_pattern): (f64, bool),
) -> Result<(), String> {
    cost.nodes += 1;
    for f in group.filters() {
        cost.filters += f.primitives().len();
        for p in f.primitives() {
            match p.kind() {
                usvg::filter::Kind::GaussianBlur(b) => {
                    cost.blur = cost.blur.max(b.std_dev_x().get()).max(b.std_dev_y().get());
                }
                usvg::filter::Kind::DropShadow(d) => {
                    cost.blur = cost.blur.max(d.std_dev_x().get()).max(d.std_dev_y().get());
                }
                _ => {}
            }
        }
    }
    // A layer of its own, drawn and then composed: a filter's region once a primitive, a clip's,
    // a mask's, an opacity's.
    if !in_pattern {
        let layers = group
            .filters()
            .iter()
            .map(|f| f.primitives().len())
            .sum::<usize>()
            + usize::from(group.clip_path().is_some())
            + usize::from(group.mask().is_some())
            + usize::from(group.opacity().get() < 1.0);
        if layers > 0 {
            let b = group.abs_layer_bounding_box();
            let px = area(b.width(), b.height(), k);
            let filters = group
                .filters()
                .iter()
                .map(|f| f.primitives().len())
                .sum::<usize>() as f64;
            cost.area += layers as f64 * px;
            cost.work += px * (filters * FILTER + (layers as f64 - filters) * LAYER);
        }
    }
    cost.over(limits)?;
    if let Some(c) = group.clip_path() {
        cost_of(c.root(), cost, pictures, limits, (k, in_pattern))?;
    }
    if let Some(m) = group.mask() {
        cost_of(m.root(), cost, pictures, limits, (k, in_pattern))?;
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => cost_of(g, cost, pictures, limits, (k, in_pattern))?,
            usvg::Node::Path(p) => {
                cost.nodes += 1;
                cost.segments += p.data().len();
                if !in_pattern {
                    // A fill: its edges once, its box; a stroke: its outline both sides of each
                    // segment and its width at each, its box.
                    let (rows, length, count) = edges_of(p.data(), p.abs_transform(), k);
                    if p.fill().is_some() {
                        let b = p.abs_bounding_box();
                        let px = area(b.width(), b.height(), k);
                        cost.edges += rows;
                        cost.area += px;
                        cost.work += rows * EDGE + px;
                    }
                    if let Some(st) = p.stroke() {
                        let t = p.abs_transform();
                        let scale = f64::from((t.sx * t.sy - t.kx * t.ky).abs().sqrt());
                        let width = f64::from(st.width().get()) * scale * k;
                        // Its outline's two sides (and each segment's ends, across its width, however
                        // level it runs), and the band it paints: its length by its width (at least a
                        // pixel: its edges are drawn however thin).
                        let rows = 2.0 * rows + count as f64 * width.max(1.0);
                        cost.edges += rows;
                        cost.area += length * width.max(1.0);
                        cost.work += rows * EDGE + length * width.max(1.0);
                    }
                }
                cost.over(limits)?;
                // Each paint with what it covers: a fill its path's box, a stroke its stroke's
                // (half its width beyond the path all round).
                for (paint, covered) in [
                    p.fill().map(|f| (f.paint(), p.bounding_box())),
                    p.stroke().map(|s| (s.paint(), p.stroke_bounding_box())),
                ]
                .into_iter()
                .flatten()
                {
                    if let usvg::Paint::Pattern(pat) = paint {
                        if in_pattern {
                            return Err("a pattern inside a pattern".into());
                        }
                        // Its tiles over what it covers, and what they draw.
                        let (r, t) = (pat.rect(), pat.transform());
                        let tile = f64::from(r.width())
                            * f64::from(r.height())
                            * f64::from((t.sx * t.sy - t.kx * t.ky).abs());
                        let tiles = if tile > 0.0 {
                            (f64::from(covered.width()) * f64::from(covered.height()) / tile)
                                .ceil()
                                .max(1.0)
                        } else {
                            f64::INFINITY
                        };
                        cost.tiles += tiles;
                        cost.tile_nodes += tiles * nodes_in(pat.root()) as f64;
                        cost.over(limits)?;
                        cost_of(pat.root(), cost, pictures, limits, (k, true))?;
                    }
                }
            }
            usvg::Node::Image(i) => {
                cost.nodes += 1;
                if !in_pattern {
                    // (A picture resampled as it is drawn: twice a fill's pixel.)
                    let b = i.abs_bounding_box();
                    let px = area(b.width(), b.height(), k);
                    cost.area += px;
                    cost.work += 2.0 * px;
                }
                if let usvg::ImageKind::PNG(data) = i.kind() {
                    pictures.push(data.clone());
                }
            }
            usvg::Node::Text(_) => cost.nodes += 1,
        }
    }
    cost.over(limits)
}

/// A pack's drawing read: its tree, within `limits`, its cost counted at its own size (its
/// longer side at most `svg_test_side` pixels) (`Err` with why not). The plug-in reads a pack's
/// drawings only through this or [`parse_at`].
pub fn parse(bytes: &[u8], limits: &Limits) -> Result<(usvg::Tree, Cost), String> {
    parse_at(bytes, limits, None)
}

/// The side, in pixels, a drawing's cost is counted at: `side` if given, else its own size's
/// longer side, at most `svg_test_side`.
fn side_of(tree: &usvg::Tree, limits: &Limits, side: Option<u32>) -> u32 {
    let size = tree.size();
    side.unwrap_or_else(|| {
        (size.width().max(size.height()).ceil() as u32).min(limits.svg_test_side)
    })
    .max(1)
}

/// A pack's drawing read as [`parse`] reads it, its cost counted drawn with its longer side
/// `side` pixels (the size a plug-in draws it at) if given.
pub fn parse_at(
    bytes: &[u8],
    limits: &Limits,
    side: Option<u32>,
) -> Result<(usvg::Tree, Cost), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_owned())?;
    scan(text, limits)?;
    let refused = Arc::default();
    // (A panic in usvg's reading, which nothing known causes now, is a refusal too.)
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        usvg::Tree::from_str(text, &options_within(*limits, Arc::clone(&refused)))
    }));
    if let Some(why) = refused.lock().unwrap_or_else(|e| e.into_inner()).take() {
        return Err(format!("an embedded picture: {why}"));
    }
    let tree = match read {
        Ok(tree) => tree.map_err(|e| format!("cannot be read: {e}"))?,
        Err(_) => return Err("cannot be read: reading it failed".into()),
    };
    let mut cost = Cost {
        buffer_bytes: memory::check(&tree, side_of(&tree, limits, side), limits)?,
        ..Cost::default()
    };
    let mut pictures = Vec::new();
    // Counted at the size it is drawn.
    let size = tree.size();
    let k = f64::from(side_of(&tree, limits, side)) / f64::from(size.width().max(size.height()));
    cost_of(tree.root(), &mut cost, &mut pictures, limits, (k, false))?;
    for p in pictures {
        let (w, h) =
            crate::picture::decode(&p, limits).map_err(|e| format!("an embedded picture: {e}"))?;
        cost.pixels += u64::from(w) * u64::from(h);
        if cost.pixels > limits.picture_pixels {
            return Err(format!(
                "its embedded pictures are more than {} pixels together",
                limits.picture_pixels
            ));
        }
    }
    Ok((tree, cost))
}

/// How a test drawing went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawn {
    /// Within its deadline, in this many milliseconds.
    In(u64),
    /// Not done by its deadline (left to finish on its own thread, its result unused).
    Late,
    /// Not drawn: an earlier test drawing ran past its deadline and is still running, or no
    /// thread could be started for it.
    Refused,
    /// The drawing has no size to draw at.
    Sizeless,
    /// Not drawn: its buffers are past the limits (which [`parse_at`] refuses first).
    TooLarge,
    /// Drawing it panicked (the panic caught on its thread).
    Panicked,
}

/// The test drawings of a process (a plug-in's instances share it).
static TEST_DRAWINGS: TestDrawings = TestDrawings::new();

/// Test drawings, each on a thread of its own: while one that ran past its deadline is still
/// running, no other is started.
struct TestDrawings {
    /// The drawings still running past their deadline.
    runaway: std::sync::atomic::AtomicUsize,
}

/// Draws `tree` with its longer side `side` pixels on a thread of its own, waiting for it no
/// longer than `deadline_ms`: the waiting itself is bounded, whatever the drawing costs. While
/// an earlier drawing runs past its deadline, none is drawn ([`Drawn::Refused`]).
pub fn test_draw(tree: &usvg::Tree, side: u32, deadline_ms: u64) -> Drawn {
    // (Its buffers checked here too, whoever calls it.)
    if memory::check(tree, side, &Limits::default()).is_err() {
        return Drawn::TooLarge;
    }
    let size = tree.size();
    let k = side as f32 / size.width().max(size.height());
    let (w, h) = (
        (size.width() * k).ceil().max(1.0) as u32,
        (size.height() * k).ceil().max(1.0) as u32,
    );
    let Some(mut pixmap) = Pixmap::new(w, h) else {
        return Drawn::Sizeless;
    };
    let tree = tree.clone();
    TEST_DRAWINGS.run(deadline_ms, move || {
        resvg::render(&tree, Transform::from_scale(k, k), &mut pixmap.as_mut());
    })
}

impl TestDrawings {
    const fn new() -> Self {
        Self {
            runaway: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// `draw` on a thread of its own, waited for no longer than `deadline_ms`.
    fn run(&'static self, deadline_ms: u64, draw: impl FnOnce() + Send + 'static) -> Drawn {
        use std::sync::atomic::Ordering;
        if self.runaway.load(Ordering::Acquire) > 0 {
            return Drawn::Refused;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        // Whether the drawing ended within its deadline. Whichever of the drawing's thread (as
        // it ends, however: drawn or panicked) and the waiting caller (at the deadline) marks
        // it second knows the drawing ran late and has ended: it takes it off `runaway`.
        let late = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let late_here = Arc::clone(&late);
        let spawned = std::thread::Builder::new()
            .name("skin-pack test drawing".into())
            .spawn(move || {
                let t = Instant::now();
                let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(draw))
                    .map(|()| t.elapsed().as_millis() as u64);
                let _ = tx.send(drawn);
                if late_here.swap(true, Ordering::AcqRel) {
                    self.runaway.fetch_sub(1, Ordering::AcqRel);
                }
            });
        if spawned.is_err() {
            return Drawn::Refused;
        }
        match rx.recv_timeout(std::time::Duration::from_millis(deadline_ms.max(1))) {
            Ok(Ok(ms)) => Drawn::In(ms),
            // (The thread sends before it ends, whatever drawing does: a lost answer is a
            // drawing that did not end well.)
            Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Drawn::Panicked,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Counted first, then marked late: if the drawing ended in between, it marked
                // it first, and nothing is running.
                self.runaway.fetch_add(1, Ordering::AcqRel);
                if late.swap(true, Ordering::AcqRel) {
                    self.runaway.fetch_sub(1, Ordering::AcqRel);
                }
                Drawn::Late
            }
        }
    }
}

/// How much larger than [`Limits`] a drawing's buffers may be as the plug-in draws it: it may
/// be drawn larger than it was checked at (a large window on a high-resolution screen), and
/// stretched from its own proportions to a part's.
pub const DRAWN_MARGIN: u64 = 4;

/// Draws a pack's drawing `tree` by `transform` into `pixmap`, as the plug-in draws one: only
/// if its buffers, at that transform and size, are within [`DRAWN_MARGIN`] times the default
/// [`Limits`], and a panic in drawing it caught. Whether it was drawn whole (if not, `pixmap`
/// may hold part of it).
pub fn render(
    tree: &usvg::Tree,
    transform: Transform,
    pixmap: &mut resvg::tiny_skia::PixmapMut<'_>,
) -> bool {
    let limits = Limits::default();
    let most = Limits {
        svg_buffer_pixels: limits.svg_buffer_pixels * DRAWN_MARGIN,
        svg_buffer_bytes: limits.svg_buffer_bytes * DRAWN_MARGIN,
        ..limits
    };
    if memory::check_at(tree, transform, (pixmap.width(), pixmap.height()), &most).is_err() {
        return false;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        resvg::render(tree, transform, pixmap);
    }))
    .is_ok()
}

/// Checks a drawing of a pack, drawing it as a test if `test`: the pixels of the pictures
/// embedded in it (counted with the pack's), if it is not refused.
pub fn check(path: &str, bytes: &[u8], limits: &Limits, test: bool, report: &mut Report) -> u64 {
    check_with(path, bytes, limits, test, report, test_draw)
}

fn check_with(
    path: &str,
    bytes: &[u8],
    limits: &Limits,
    test: bool,
    report: &mut Report,
    draw: impl FnOnce(&usvg::Tree, u32, u64) -> Drawn,
) -> u64 {
    let parsed = parse(bytes, limits);
    let pixels = parsed.as_ref().map_or(0, |(_, cost)| cost.pixels);
    match parsed {
        Err(why) => report.reject("svg.refused", Some(path), why),
        // (Waited for twice its limit, so that a drawing just over it is measured.)
        Ok((tree, _)) if test => {
            let side = side_of(&tree, limits, None);
            match draw(&tree, side, limits.svg_test_ms * 2 + 1) {
                Drawn::In(ms) if ms <= limits.svg_test_ms => {}
                Drawn::In(ms) => report.reject(
                    "svg.slow",
                    Some(path),
                    format!(
                        "drawn {side} pixels across in {ms} ms, more than {} ms",
                        limits.svg_test_ms
                    ),
                ),
                Drawn::Late => report.reject(
                    "svg.slow",
                    Some(path),
                    format!(
                        "not drawn {side} pixels across within {} ms",
                        limits.svg_test_ms * 2 + 1
                    ),
                ),
                // (Its cost is bounded by its counted work, which it passed: the timed test is
                // a backstop, and another pack's runaway does not refuse this one.)
                Drawn::Refused => report.note(
                    "svg.untested",
                    Some(path),
                    "not drawn as a test: an earlier drawing's test is still running past its \
                     deadline (its counted cost is within the limit)",
                ),
                Drawn::Sizeless => {
                    report.reject("svg.size", Some(path), "it has no size to draw at")
                }
                Drawn::TooLarge => report.reject(
                    "svg.refused",
                    Some(path),
                    "its drawing buffers are past the limits",
                ),
                // (Refused for itself: the backstop stays on for the packs after it.)
                Drawn::Panicked => {
                    report.reject("svg.panicked", Some(path), "drawing it as a test panicked")
                }
            }
        }
        Ok(_) => {}
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svg(body: &str) -> String {
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' \
             viewBox='0 0 100 100' width='100' height='100'>{body}</svg>"
        )
    }

    fn refused_whole(drawing: &str) -> String {
        parse(drawing.as_bytes(), &Limits::default())
            .err()
            .unwrap_or_else(|| panic!("accepted: {drawing}"))
    }

    fn refused(body: &str) -> String {
        parse(svg(body).as_bytes(), &Limits::default())
            .err()
            .unwrap_or_else(|| panic!("accepted: {body}"))
    }

    #[test]
    fn a_plain_drawing_is_read() {
        let s = svg(
            "<defs><linearGradient id='g'><stop offset='0' stop-color='#000'/></linearGradient>\
             <filter id='f'><feGaussianBlur stdDeviation='2'/></filter></defs>\
             <rect width='100' height='100' fill='url(#g)' filter='url(#f)'/>\
             <circle cx='50' cy='50' r='20' style='fill:url(#g)'/>",
        );
        let (_, cost) = parse(s.as_bytes(), &Limits::default()).expect("read");
        assert_eq!(cost.filters, 1);
        assert_eq!(cost.blur, 2.0);
    }

    #[test]
    fn code_animation_and_text_are_refused() {
        assert!(refused("<script>alert(1)</script>").contains("no code"));
        assert!(refused("<foreignObject><div/></foreignObject>").contains("no code"));
        assert!(refused("<rect width='1' height='1' onclick='x()'/>").contains("event handler"));
        assert!(refused("<rect><animate attributeName='x'/></rect>").contains("animation"));
        assert!(refused("<text>MOOG</text>").contains("legends"));
        assert!(refused("<style>rect{fill:red}</style>").contains("style sheet"));
    }

    /// `metadata` holds other namespaces' data, never SVG's: usvg reads a style sheet wherever
    /// it is, so one hidden there would be applied unchecked.
    #[test]
    fn nothing_of_svgs_hides_in_metadata() {
        assert!(
            refused("<metadata><style>rect{fill:url(x.svg#a)}</style></metadata>")
                .contains("style sheet")
        );
        assert!(
            refused("<metadata><x:style xmlns:x='urn:x'>rect{}</x:style></metadata>")
                .contains("style sheet")
        );
        assert!(refused("<metadata><rect width='1' height='1'/></metadata>").contains("metadata"));
        // A licence's RDF stays allowed.
        let rdf = svg(
            "<metadata><rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>             <rdf:Description/></rdf:RDF></metadata><rect width='9' height='9'/>",
        );
        assert!(parse(rdf.as_bytes(), &Limits::default()).is_ok());
    }

    /// Patterns filled with patterns, ten references a level: counted as they repeat, and a
    /// pattern inside a pattern refused.
    #[test]
    fn patterns_that_multiply_are_refused() {
        let mut body = String::from(
            "<defs><pattern id='p0' width='1' height='1' patternUnits='userSpaceOnUse'>\
             <rect width='1' height='1'/></pattern>",
        );
        for k in 1..7 {
            body.push_str(&format!(
                "<pattern id='p{k}' width='10' height='10' patternUnits='userSpaceOnUse'>"
            ));
            for _ in 0..10 {
                body.push_str(&format!(
                    "<rect width='10' height='10' fill='url(#p{})'/>",
                    k - 1
                ));
            }
            body.push_str("</pattern>");
        }
        body.push_str("</defs><rect width='100' height='100' fill='url(#p6)'/>");
        assert!(
            refused(&body).contains("elements drawn"),
            "{}",
            refused(&body)
        );
        let nested = "<defs><pattern id='a' width='5' height='5' patternUnits='userSpaceOnUse'>\
                      <rect width='5' height='5'/></pattern>\
                      <pattern id='b' width='10' height='10' patternUnits='userSpaceOnUse'>\
                      <rect width='10' height='10' fill='url(#a)'/></pattern></defs>\
                      <rect width='100' height='100' fill='url(#b)'/>";
        assert!(refused(nested).contains("pattern inside a pattern"));
    }

    /// Ids resolve as usvg resolves them: one given twice, or hidden in `metadata`, is refused.
    #[test]
    fn ids_are_given_once_and_only_to_svgs_elements() {
        assert!(refused("<rect id='a'/><circle id='a'/>").contains("two elements"));
        assert!(refused("<rect xml:id='p' id='q'/><rect fill='url(#p)'/>").contains("namespace"));
        assert!(
            refused("<metadata><x:d xmlns:x='urn:x' id='p'/></metadata><rect fill='url(#p)'/>")
                .contains("inside `metadata`")
        );
    }

    /// A pattern's tiles under a stroke are counted over the stroke, however thin the path.
    #[test]
    fn a_stroked_patterns_tiles_are_counted_over_the_stroke() {
        let line = "<defs><pattern id='p' width='0.1' height='0.1' patternUnits='userSpaceOnUse'>\
                    <rect width='0.1' height='0.1'/></pattern></defs>\
                    <line x1='0' y1='50' x2='100' y2='50' stroke='url(#p)' stroke-width='60'/>";
        assert!(refused(line).contains("tiles"), "{}", refused(line));
    }

    /// A tiny tile over a large fill: millions of tiles, refused before anything is drawn.
    #[test]
    fn a_patterns_tiles_are_counted() {
        let tiny = "<defs><pattern id='p' width='0.01' height='0.01' patternUnits='userSpaceOnUse'>\
                    <rect width='0.01' height='0.01'/></pattern></defs>\
                    <rect width='100' height='100' fill='url(#p)'/>";
        assert!(refused(tiny).contains("tiles"));
        let fine = "<defs><pattern id='p' width='10' height='10' patternUnits='userSpaceOnUse'>\
                    <rect width='5' height='5'/></pattern></defs>\
                    <rect width='100' height='100' fill='url(#p)'/>";
        assert!(parse(svg(fine).as_bytes(), &Limits::default()).is_ok());
    }

    #[test]
    fn references_outside_the_drawing_are_refused() {
        assert!(refused("<image href='/etc/passwd' width='1' height='1'/>").contains("refers"));
        assert!(
            refused("<image xlink:href='https://example.com/x.png' width='1' height='1'/>")
                .contains("refers")
        );
        assert!(refused("<image href='data:image/svg+xml;base64,AAAA'/>").contains("refers"));
        assert!(refused("<use href='other.svg#a'/>").contains("refers"));
        assert!(refused("<rect fill='url(other.svg#g)'/>").contains("outside"));
        assert!(refused("<rect style=\"fill:url('file:///x')\"/>").contains("outside"));
    }

    #[test]
    fn a_dtd_and_its_entities_are_refused() {
        let s = "<?xml version='1.0'?><!DOCTYPE svg [<!ENTITY a 'aaaaaaaaaa'><!ENTITY b '&a;&a;&a;&a;'>]>\
                 <svg xmlns='http://www.w3.org/2000/svg'>&b;</svg>";
        assert!(parse(s.as_bytes(), &Limits::default()).is_err());
    }

    /// Ten `use`s of ten `use`s of ... : a few hundred bytes drawing millions of shapes.
    #[test]
    fn a_pattern_filled_with_itself_by_inheritance_is_refused_before_usvg_reads_it() {
        // (usvg's reading recursed until its stack overflowed, which aborts: checked here by
        // the scan alone, which runs before it.)
        let fuzzed = std::str::from_utf8(include_bytes!(
            "../tests/fixtures/inherited-pattern-fuzz.svg"
        ))
        .expect("text");
        let pattern = "<pattern id='p' width='10' height='10' patternUnits='userSpaceOnUse'>";
        let used = "<rect width='10' height='10' fill='url(#p)'/>";
        for drawing in [
            fuzzed.to_owned(),
            svg(&format!(
                "<g fill='url(#p)'>{pattern}<rect width='5' height='5'/></pattern></g>\
                 <rect width='10' height='10' fill='url(#p)'/>"
            )),
            svg(&format!(
                "<g style='stroke: url(#p)'><g fill='inherit'>{pattern}\
                 <rect width='5' height='5'/></pattern></g></g>{used}"
            )),
            svg(&format!(
                "<rect id='r' width='5' height='5'/><g fill='url(#p)'>{pattern}\
                 <use href='#r'/></pattern></g>{used}"
            )),
            svg(&format!(
                "<g fill='url(#q)'>{}<rect width='5' height='5'/></pattern></g>\
                 {pattern}<rect width='5' height='5' fill='url(#q)'/></pattern>{used}",
                pattern.replace("'p'", "'q'")
            )),
        ] {
            let why = scan(&drawing, &Limits::default()).unwrap_err();
            assert!(why.contains("repeats itself"), "{drawing}: {why}");
        }
        // A pattern's own fill, or one its elements set themselves, is not its own.
        for drawing in [
            svg(&format!(
                "<g fill='url(#p)'>{pattern}<rect width='5' height='5' fill='red'/></pattern>\
                 </g><rect width='10' height='10' fill='url(#p)'/>"
            )),
            svg(&format!(
                "<g fill='red'>{pattern}<rect width='5' height='5'/></pattern></g>\
                 <rect width='10' height='10' fill='url(#p)'/>"
            )),
        ] {
            parse(drawing.as_bytes(), &Limits::default()).expect("a pattern filled red");
        }
    }

    #[test]
    fn an_inherited_pattern_is_found_whichever_declaration_wins() {
        // A pattern's rectangle whose own declarations end in `inherit`: the declaration that
        // wins as usvg resolves them, and as CSS cascades them (a review of R-SKIN-PACKS).
        let pattern = "<pattern id='p' width='10' height='10' patternUnits='userSpaceOnUse'>";
        let inside = |above: &str, rect: &str| {
            svg(&format!(
                "<g {above}>{pattern}<rect width='5' height='5' {rect}/></pattern></g>\
                 <rect width='10' height='10' fill='url(#p)'/>"
            ))
        };
        let fill = "fill='url(#p)'";
        for drawing in [
            // The later declaration wins (the review's case), for a stroke too.
            inside(fill, "style='fill:red; fill:inherit'"),
            inside("stroke='url(#p)'", "style='stroke: red ; stroke: inherit'"),
            // The style over the attribute.
            inside(fill, "fill='red' style='fill:inherit'"),
            // usvg: an inherited value keeps its ancestor's `!important`, and the first
            // `!important` stays over a later declaration.
            inside(
                "style='fill:url(#p) !important'",
                "style='fill:inherit; fill:red'",
            ),
            // CSS: `!important` over a later declaration.
            inside(fill, "style='fill:inherit !important; fill:red'"),
        ] {
            let why = scan(&drawing, &Limits::default()).unwrap_err();
            assert!(why.contains("repeats itself"), "{drawing}: {why}");
        }
        // Red wins in both.
        for drawing in [
            inside(fill, "style='fill:inherit; fill:red'"),
            inside(fill, "fill='inherit' style='fill:red'"),
            inside(fill, "style='fill:red !important; fill:inherit'"),
            inside(fill, "style='fill:red; /* fill:inherit */'"),
        ] {
            parse(drawing.as_bytes(), &Limits::default()).expect("a pattern filled red");
        }
    }

    #[test]
    fn uses_that_multiply_are_counted_as_drawn() {
        let mut body = String::from("<defs><rect id='l0' width='1' height='1'/>");
        for k in 1..8 {
            body.push_str(&format!("<g id='l{k}'>"));
            for _ in 0..10 {
                body.push_str(&format!("<use href='#l{}'/>", k - 1));
            }
            body.push_str("</g>");
        }
        body.push_str("</defs><use href='#l7'/>");
        assert!(refused(&body).contains("elements drawn"));
        assert!(refused("<g id='a'><use href='#a'/></g>").contains("repeats itself"));
        assert!(
            refused("<defs><linearGradient id='g' href='#g'/></defs>").contains("repeats itself")
        );
    }

    #[test]
    fn costs_past_their_limits_are_refused() {
        let mut d = String::from("M0 0");
        for i in 0..2000 {
            d.push_str(&format!(" L{} {}", i % 100, i % 97));
        }
        let limits = Limits {
            svg_segments: 1000,
            ..Limits::default()
        };
        let s = svg(&format!("<path d='{d}'/>"));
        assert!(
            parse(s.as_bytes(), &limits)
                .unwrap_err()
                .contains("segments")
        );
        let blur = svg(
            "<filter id='f'><feGaussianBlur stdDeviation='500'/></filter><rect width='9' height='9' filter='url(#f)'/>",
        );
        assert!(
            parse(blur.as_bytes(), &Limits::default())
                .unwrap_err()
                .contains("blur")
        );
    }

    #[test]
    fn a_slow_drawing_is_refused_by_its_test() {
        // (Its own size 2000 pixels: drawn so as its test.)
        let s = svg(
            "<filter id='f'><feGaussianBlur stdDeviation='60'/></filter>\
             <rect width='100' height='100' filter='url(#f)'/>",
        )
        .replace("width='100' height='100'>", "width='2000' height='2000'>");
        let mut r = Report::default();
        // (Its cost not capped here: the timed test, the backstop, is what is tested.)
        let limits = Limits {
            svg_test_ms: 0,
            svg_test_side: 2000,
            svg_work: f64::INFINITY,
            ..Limits::default()
        };
        check("art/slow.svg", s.as_bytes(), &limits, true, &mut r);
        assert!(r.has("svg.slow"), "{r}");
    }

    /// Test drawings of their own, apart from the process's.
    fn drawings() -> &'static TestDrawings {
        Box::leak(Box::new(TestDrawings::new()))
    }

    #[test]
    fn a_test_drawing_that_panics_refuses_its_pack_and_the_next_is_tested() {
        // (Before: the panic was taken for a drawing past its deadline, "not drawn within 4001
        // ms", and every later pack's test was skipped for as long as the process ran.)
        let drawings = drawings();
        let s = svg("<rect width='10' height='10'/>");
        let limits = Limits::default();
        let mut report = Report::default();
        check_with(
            "art/panics.svg",
            s.as_bytes(),
            &limits,
            true,
            &mut report,
            |_, _, ms| drawings.run(ms, || panic!("a test drawing panics")),
        );
        assert!(report.has("svg.panicked"), "{report}");
        assert!(!report.has("svg.slow"), "{report}");
        assert!(!report.to_string().contains("within"), "{report}");
        let drawn = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let drawn_here = Arc::clone(&drawn);
        let mut next = Report::default();
        check_with(
            "art/next.svg",
            s.as_bytes(),
            &limits,
            true,
            &mut next,
            |_, _, ms| {
                drawings.run(ms, move || {
                    drawn_here.store(true, std::sync::atomic::Ordering::Release);
                })
            },
        );
        assert!(drawn.load(std::sync::atomic::Ordering::Acquire));
        assert!(next.findings.is_empty(), "{next}");
    }

    #[test]
    fn a_drawing_that_panics_after_its_deadline_releases_the_backstop() {
        use std::sync::atomic::Ordering;
        let drawings = drawings();
        let (finish, wait) = std::sync::mpsc::channel::<()>();
        let late = drawings.run(1, move || {
            let _ = wait.recv();
            panic!("a late test drawing panics");
        });
        assert_eq!(late, Drawn::Late);
        assert_eq!(drawings.run(1000, || {}), Drawn::Refused);
        finish.send(()).expect("the drawing is waiting");
        let until = Instant::now() + std::time::Duration::from_secs(10);
        while drawings.runaway.load(Ordering::Acquire) > 0 && Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(matches!(drawings.run(1000, || {}), Drawn::In(_)));
    }

    #[test]
    fn two_drawings_past_their_deadline_both_hold_the_backstop() {
        use std::sync::atomic::Ordering;
        let drawings = drawings();
        let wait_for = |n: usize| {
            let until = Instant::now() + std::time::Duration::from_secs(10);
            while drawings.runaway.load(Ordering::Acquire) > n && Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        let (finish_a, wait_a) = std::sync::mpsc::channel::<()>();
        let (finish_b, wait_b) = std::sync::mpsc::channel::<()>();
        // Both started before either is known to be late (as two instances' checks may be).
        let a = std::thread::spawn(move || {
            drawings.run(50, move || {
                let _ = wait_a.recv();
            })
        });
        let b = std::thread::spawn(move || {
            drawings.run(50, move || {
                let _ = wait_b.recv();
            })
        });
        let (a, b) = (a.join().expect("a"), b.join().expect("b"));
        assert!(a == Drawn::Late || b == Drawn::Late, "{a:?} {b:?}");
        if a == Drawn::Late && b == Drawn::Late {
            // The first ends; the second still runs, and the backstop holds.
            drop(finish_a);
            wait_for(1);
            assert_eq!(drawings.run(1000, || {}), Drawn::Refused);
            drop(finish_b);
        } else {
            drop((finish_a, finish_b));
        }
        wait_for(0);
        assert!(matches!(drawings.run(1000, || {}), Drawn::In(_)));
    }

    #[test]
    fn a_vast_pattern_tile_is_refused_at_every_size_it_is_drawn() {
        let limits = Limits::default();
        for bytes in [
            include_bytes!("../tests/fixtures/vast-pattern/art/panel.svg").as_slice(),
            include_bytes!("../tests/fixtures/pattern-fuzz-17237.svg").as_slice(),
        ] {
            for side in [200, 300, 3000] {
                let why = parse_at(bytes, &limits, Some(side)).unwrap_err();
                assert!(why.contains("a pattern's tile"), "{why}");
            }
            // Whoever draws it: the test drawing and the plug-in's drawing check it as well.
            let tree = usvg::Tree::from_data(bytes, &options()).expect("a valid SVG");
            assert_eq!(test_draw(&tree, 300, 1000), Drawn::TooLarge);
            let mut pixmap = Pixmap::new(300, 150).expect("a pixmap");
            assert!(!render(
                &tree,
                Transform::from_scale(1.5, 1.5),
                &mut pixmap.as_mut()
            ));
        }
        let s = svg(
            "<pattern id='p' width='200' height='200' patternUnits='userSpaceOnUse'>\
             <rect width='1' height='1'/></pattern>\
             <rect width='1' height='1' fill='url(#p)'/>",
        );
        assert!(parse_at(s.as_bytes(), &limits, Some(100)).is_ok());
        let why = parse_at(s.as_bytes(), &limits, Some(3000)).unwrap_err();
        assert!(why.contains("a pattern's tile"), "{why}");
    }

    #[test]
    fn a_pattern_tile_is_counted_at_its_transforms_scale() {
        for (transform, ancestors) in [
            ("scale(100000)", ""),
            ("matrix(1 0 1000000 1 0 0)", ""),
            ("", "scale(100000)"),
        ] {
            let s = svg(&format!(
                "<pattern id='p' width='10' height='10' patternUnits='userSpaceOnUse' \
                 patternTransform='{transform}'><rect width='1' height='1'/></pattern>\
                 <g transform='{ancestors}'><rect width='.001' height='.001' fill='url(#p)'/></g>"
            ));
            let why = parse(s.as_bytes(), &Limits::default()).unwrap_err();
            assert!(
                why.contains("a pattern's tile"),
                "{transform} {ancestors}: {why}"
            );
        }
    }

    #[test]
    fn filter_mask_and_layer_buffers_are_bounded_whatever_the_work() {
        let limits = Limits {
            svg_work: f64::INFINITY,
            ..Limits::default()
        };
        for body in [
            // A filter's region is not cut to the drawing.
            "<filter id='f' filterUnits='userSpaceOnUse' width='100000' height='100000'>\
             <feFlood/></filter><rect width='1' height='1' filter='url(#f)'/>",
            "<filter id='f'><feFlood x='-1000000000' width='1000000000'/></filter>\
             <rect width='1' height='1' filter='url(#f)'/>",
            "<mask id='m' maskUnits='userSpaceOnUse' width='1000000000' height='100'>\
             <rect width='1' height='1'/></mask><rect width='1' height='1' mask='url(#m)'/>",
            "<g opacity='0.5'><rect width='1000000000' height='100'/></g>",
        ] {
            let why = parse(svg(body).as_bytes(), &limits).unwrap_err();
            assert!(
                why.contains("drawing buffer") || why.contains("integer range"),
                "{body}: {why}"
            );
        }
    }

    #[test]
    fn a_layer_is_counted_as_resvg_cuts_it() {
        // A translucent group far wider than the drawing: resvg cuts its layer to five times
        // the drawing's size, and so is it counted (here within the limits).
        let limits = Limits {
            svg_work: f64::INFINITY,
            ..Limits::default()
        };
        let (_, cost) = parse(
            svg("<g opacity='.5'><rect x='-100000' width='200000' height='100'/></g>").as_bytes(),
            &limits,
        )
        .expect("a layer cut to the drawing");
        assert!(cost.buffer_bytes <= (100 * 100 + 500 * 104) * 4, "{cost:?}");
    }

    #[test]
    fn chained_clips_masks_and_nested_groups_count_all_their_buffers() {
        let limits = Limits {
            svg_work: f64::INFINITY,
            svg_buffer_bytes: 200_000,
            ..Limits::default()
        };
        for body in [
            "<clipPath id='a'><rect width='100' height='100'/></clipPath>\
             <clipPath id='b' clip-path='url(#a)'><rect width='100' height='100'/></clipPath>\
             <clipPath id='c' clip-path='url(#b)'><rect width='100' height='100'/></clipPath>\
             <rect width='100' height='100' clip-path='url(#c)'/>",
            "<mask id='a'><rect width='100' height='100'/></mask>\
             <mask id='b' mask='url(#a)'><rect width='100' height='100'/></mask>\
             <rect width='100' height='100' mask='url(#b)'/>",
            "<g opacity='.5'><g opacity='.5'><g opacity='.5'><g opacity='.5'>\
             <rect width='100' height='100'/></g></g></g></g>",
            "<filter id='f'><feFlood/><feGaussianBlur stdDeviation='1'/></filter>\
             <rect width='100' height='100' filter='url(#f)'/>",
        ] {
            let why = parse(svg(body).as_bytes(), &limits).unwrap_err();
            assert!(why.contains("together"), "{body}: {why}");
        }
    }

    #[test]
    fn a_pattern_in_a_chained_mask_is_counted() {
        let s = svg(
            "<pattern id='p' width='10' height='1000000000' patternUnits='userSpaceOnUse'>\
             <rect width='1' height='1'/></pattern>\
             <mask id='a'><rect width='10' height='10' fill='url(#p)'/></mask>\
             <mask id='b' mask='url(#a)'><rect width='10' height='10'/></mask>\
             <rect width='10' height='10' mask='url(#b)'/>",
        );
        let why = parse(s.as_bytes(), &Limits::default()).unwrap_err();
        assert!(why.contains("a pattern's tile"), "{why}");
    }

    #[test]
    fn a_drawing_stretched_past_its_checked_size_is_not_drawn() {
        // Within the limits as checked, at its own proportions; stretched a million times one
        // way, its pattern's tile would be past them, and the plug-in does not draw it.
        let s = svg(
            "<pattern id='p' width='50' height='50' patternUnits='userSpaceOnUse'>\
             <rect width='1' height='1'/></pattern><rect width='100' height='100' \
             fill='url(#p)'/>",
        );
        let (tree, _) = parse(s.as_bytes(), &Limits::default()).expect("within the limits");
        let mut pixmap = Pixmap::new(100, 100).expect("a pixmap");
        assert!(render(&tree, Transform::identity(), &mut pixmap.as_mut()));
        assert!(!render(
            &tree,
            Transform::from_scale(1.0, 1e6),
            &mut pixmap.as_mut()
        ));
    }

    #[test]
    fn deeply_nested_elements_are_refused_before_they_are_read() {
        // (Before: the XML parser's recursion overflowed its thread's stack, which aborts.)
        let s = svg(&format!(
            "{}<rect width='1' height='1'/>{}",
            "<g>".repeat(2000),
            "</g>".repeat(2000)
        ));
        assert!(
            parse(s.as_bytes(), &Limits::default())
                .unwrap_err()
                .contains("64 deep")
        );
        let mut body = String::from("<defs><rect id='r0' width='1' height='1'/>");
        for i in 1..100 {
            body.push_str(&format!("<g id='r{i}'><use href='#r{}'/></g>", i - 1));
        }
        body.push_str("</defs><use href='#r99'/>");
        assert!(refused(&body).contains("64 deep"));
    }

    #[test]
    fn a_drawing_nested_as_deep_as_allowed_is_read_and_drawn_on_a_small_stack() {
        // Windows gives a thread 1 MB by default: a host's editor thread may have no more.
        let deep = |depth: usize| {
            svg(&format!(
                "<clipPath id='c'><rect width='90' height='90'/></clipPath>{}\
                 <rect width='1' height='1' fill='#123'/>{}",
                "<g opacity='.9' clip-path='url(#c)'>".repeat(depth),
                "</g>".repeat(depth)
            ))
        };
        // References followed count as deep: a `use` of a group of a `use`..., and a clip
        // clipped by a clip...
        let uses = |depth: usize| {
            let mut body = String::from("<defs><rect id='u0' width='1' height='1'/>");
            for i in 1..=depth {
                body.push_str(&format!(
                    "<g id='u{i}' opacity='.9'><use href='#u{}'/></g>",
                    i - 1
                ));
            }
            svg(&format!("{body}</defs><use href='#u{depth}'/>"))
        };
        let clips = |depth: usize| {
            let mut body =
                String::from("<clipPath id='c0'><rect width='90' height='90'/></clipPath>");
            for i in 1..=depth {
                body.push_str(&format!(
                    "<clipPath id='c{i}' clip-path='url(#c{})'><rect width='90' height='90'/>\
                     </clipPath>",
                    i - 1
                ));
            }
            svg(&format!(
                "{body}<rect width='9' height='9' clip-path='url(#c{depth})'/>"
            ))
        };
        for drawing in [deep(60), uses(30), clips(60)] {
            let read = std::thread::Builder::new()
                .stack_size(1 << 20)
                .spawn(move || {
                    let (tree, _) = parse(drawing.as_bytes(), &Limits::default())?;
                    let mut pixmap = Pixmap::new(100, 100).expect("a pixmap");
                    Ok::<bool, String>(render(&tree, Transform::identity(), &mut pixmap.as_mut()))
                })
                .expect("a thread")
                .join()
                .expect("read and drawn");
            assert_eq!(read, Ok(true));
        }
        for drawing in [deep(70), uses(40), clips(70)] {
            assert!(refused_whole(&drawing).contains("64 deep"), "{drawing}");
        }
    }

    #[test]
    fn nesting_is_counted_past_comments_instructions_cdata_and_quoted_attributes() {
        let s = svg(&format!(
            "<!-- {} --><?example {}?><metadata><![CDATA[{}]]></metadata>\
             <rect width='1' height='1' data-example='{}' />",
            "<g>".repeat(100),
            "<g>".repeat(100),
            "<g>".repeat(100),
            ">".repeat(100)
        ));
        depth_of(&s).expect("nothing nested");
        // (Its instruction is refused by the scan, as before.)
        let s = s.replace(&format!("<?example {}?>", "<g>".repeat(100)), "");
        parse(s.as_bytes(), &Limits::default()).expect("nothing nested");
    }

    #[test]
    fn vast_filter_offsets_are_refused() {
        let why = parse(
            include_bytes!("../tests/fixtures/offset-fuzz-2222.svg"),
            &Limits::default(),
        )
        .unwrap_err();
        assert!(why.contains("a filter's offset"), "{why}");
        for primitive in ["feOffset", "feDropShadow"] {
            for offset in ["1e30", "-1e30"] {
                let s = svg(&format!(
                    "<filter id='f'><{primitive} dx='{offset}'/></filter>\
                     <rect width='1' height='1' filter='url(#f)'/>"
                ));
                let why = parse(s.as_bytes(), &Limits::default()).unwrap_err();
                assert!(why.contains("a filter's offset"), "{why}");
            }
        }
    }

    #[test]
    fn an_embedded_picture_is_counted_at_its_own_size() {
        let (_, cost) = parse(
            include_bytes!("../tests/fixtures/embedded-png.svg"),
            &Limits::default(),
        )
        .expect("a small embedded PNG");
        assert_eq!(cost.pixels, 1);
        assert_eq!(cost.buffer_bytes, (100 * 100 + 3) * 4);
        // A header claiming a vast picture: refused before anything decodes it.
        let why = parse(
            include_bytes!("../tests/fixtures/embedded-png-huge.svg"),
            &Limits::default(),
        )
        .unwrap_err();
        assert!(why.contains("an embedded picture"), "{why}");
    }
}
