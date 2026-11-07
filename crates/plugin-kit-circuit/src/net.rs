//! The network: an instrument's parts ([`crate::part::Part`]) as node equations, the way its
//! lab's netlists give them to ngspice:
//!
//! `C dx/dt + G x = Bu u + Σ P_k f_k(Q_k x + Qu_k u)`
//!
//! `x` the unknown node voltages (the circuit's nodes and the devices' and lossy capacitors'
//! internal ones), `u` the inputs (`u[0] = 1` carries the rails and constant currents; the
//! others are the ideal sources the CPU side becomes and the noise voltage), and each nonlinear
//! device `f_k` a function of its controlling voltages whose outputs are currents (and
//! charges, whose currents are their time derivatives) injected by `P_k`. Every resistor,
//! capacitor, the op-amps' linear parts and the controlled sources are in `G` and `C`; SPICE's
//! `rshunt` is on every node.

use crate::dev::{self, Dev};
use crate::devices as dv;
use crate::la::Mat;
use crate::part::{Kind, Part};
use crate::sim;
use std::collections::HashMap;

/// A slot's role (`net::Elem::role`, `ss::Slot::role`): one of the few the network's builder
/// gives, read back as that same static text. A type of its own, since serde takes a
/// `&'static str` field as borrowed from what it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Role(pub &'static str);

/// Every role the network's builder gives an element.
pub const ROLES: [&str; 7] = ["", "bc", "eb", "in", "clamp", "pot 3-2", "pot 2-1"];

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl serde::Serialize for Role {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Role {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Role, D::Error> {
        use serde::de::Error;
        let r = String::deserialize(d)?;
        ROLES
            .iter()
            .find(|&&k| k == r)
            .map(|&k| Role(k))
            .ok_or_else(|| D::Error::custom(format!("an unknown role {r:?}")))
    }
}

/// What the network takes from the instrument besides its parts: its device cards and its
/// ideal rails (each a node name and its voltage).
#[derive(Debug, Clone, Default)]
pub struct Circuit {
    pub library: dv::Library,
    pub rails: Vec<(String, f64)>,
}

/// A linear form over the unknowns and the inputs.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Lin {
    pub x: Vec<(usize, f64)>,
    pub u: Vec<(usize, f64)>,
}

impl Lin {
    fn scaled(&self, s: f64) -> Lin {
        Lin {
            x: self.x.iter().map(|&(i, c)| (i, c * s)).collect(),
            u: self.u.iter().map(|&(i, c)| (i, c * s)).collect(),
        }
    }

    fn plus(&self, b: &Lin) -> Lin {
        let mut r = self.clone();
        r.x.extend_from_slice(&b.x);
        r.u.extend_from_slice(&b.u);
        r
    }

    fn minus(&self, b: &Lin) -> Lin {
        self.plus(&b.scaled(-1.0))
    }

    fn constant(c: f64) -> Lin {
        Lin {
            x: vec![],
            u: vec![(0, c)],
        }
    }

    pub fn eval(&self, x: &[f64], u: &[f64]) -> f64 {
        self.x.iter().map(|&(i, c)| c * x[i]).sum::<f64>()
            + self.u.iter().map(|&(i, c)| c * u[i]).sum::<f64>()
    }
}

/// A nonlinear device in the network.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Elem {
    /// The part it comes from (`Q46`), and what it is of the part (`""`, `"in"`, `"clamp"`).
    pub refdes: String,
    pub section: String,
    pub role: Role,
    pub dev: Dev,
    /// For a panel pot's section: the pot's total resistance, its taper, and which section
    /// (`true`: terminal 3 to the wiper).
    pub pot: Option<(f64, crate::part::Taper, bool)>,
    /// The controlling voltages.
    pub ctrl: Vec<Lin>,
    /// Each output's injection into the unknowns (a current leaving a node is negative): the
    /// static currents first, then the charges.
    pub inj: Vec<Vec<(usize, f64)>>,
}

/// The network's equations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Net {
    /// The unknowns' names (a section's local nodes with its prefix, as the netlists name
    /// them; internal nodes after their element).
    pub names: Vec<String>,
    /// The inputs' names: `1`, then the sources.
    pub inputs: Vec<String>,
    pub g: Mat,
    pub c: Mat,
    pub bu: Mat,
    pub elems: Vec<Elem>,
}

/// What the network is built from besides the parts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Opts {
    /// The time-varying sources' nodes (each becomes an input).
    pub sources: Vec<String>,
    /// The noise voltage: in series with this part's (section id, refdes) emitter junction, as
    /// the instrument's reference splits that emitter in its netlist.
    pub noise: Option<(String, String)>,
    /// The run's RELTOL (ngspice matches a diode's breakdown to it).
    pub reltol: f64,
    /// The film capacitors' loss.
    pub cap_loss: sim::CapLoss,
    /// Nodes held at fixed voltages (an `.ic` while the operating point is found), by name:
    /// they are inputs too.
    pub held: Vec<String>,
    /// The panel pots as conductances the knobs set in real time (D41), not fixed resistors.
    pub live_pots: bool,
    /// Transistors modelled behaviourally (D58), by (section, refdes): their junctions so many
    /// times softer, matched at so much current (`BjtP::soft`).
    pub soft: Vec<(String, String, f64, f64)>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            sources: Vec::new(),
            noise: None,
            reltol: 1e-5,
            cap_loss: sim::CapLoss::NONE,
            held: Vec::new(),
            live_pots: false,
            soft: Vec::new(),
        }
    }
}

/// How a node resolves.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
enum Node {
    Ground,
    Known(f64),
    Input(usize),
    Unknown(usize),
}

struct Builder {
    names: Vec<String>,
    index: HashMap<String, usize>,
    inputs: Vec<String>,
    input_index: HashMap<String, usize>,
    rails: HashMap<String, f64>,
    library: dv::Library,
    // Stamps, collected sparse and assembled at the end.
    g: Vec<(usize, usize, f64)>,
    c: Vec<(usize, usize, f64)>,
    bu: Vec<(usize, usize, f64)>,
    elems: Vec<Elem>,
}

impl Builder {
    fn node(&mut self, name: &str) -> Node {
        if name == "0" {
            return Node::Ground;
        }
        if let Some(&v) = self.rails.get(name) {
            return Node::Known(v);
        }
        if let Some(&k) = self.input_index.get(name) {
            return Node::Input(k);
        }
        if let Some(&i) = self.index.get(name) {
            return Node::Unknown(i);
        }
        let i = self.names.len();
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), i);
        Node::Unknown(i)
    }

    fn internal(&mut self, name: String) -> usize {
        assert!(!self.index.contains_key(&name), "{name} twice");
        let i = self.names.len();
        self.index.insert(name.clone(), i);
        self.names.push(name);
        i
    }

    fn lin(&self, n: Node) -> Lin {
        match n {
            Node::Ground => Lin::default(),
            Node::Known(v) => Lin::constant(v),
            Node::Input(k) => Lin {
                x: vec![],
                u: vec![(k, 1.0)],
            },
            Node::Unknown(i) => Lin {
                x: vec![(i, 1.0)],
                u: vec![],
            },
        }
    }

    /// A conductance between two nodes.
    fn cond(&mut self, a: Node, b: Node, g: f64) {
        for (p, q) in [(a, b), (b, a)] {
            if let Node::Unknown(i) = p {
                self.g.push((i, i, g));
                match q {
                    Node::Unknown(j) => self.g.push((i, j, -g)),
                    Node::Known(v) => self.bu.push((i, 0, g * v)),
                    Node::Input(k) => self.bu.push((i, k, g)),
                    Node::Ground => {}
                }
            }
        }
    }

    /// A capacitance between two nodes (neither a time-varying source).
    fn cap(&mut self, a: Node, b: Node, c: f64, what: &str) {
        for (p, q) in [(a, b), (b, a)] {
            if let Node::Unknown(i) = p {
                self.c.push((i, i, c));
                match q {
                    Node::Unknown(j) => self.c.push((i, j, -c)),
                    Node::Input(_) => panic!("{what}: a capacitor on a time-varying source"),
                    _ => {}
                }
            }
        }
    }

    /// A current `i(u)` (a linear form in the inputs) from node `a` into node `b`, through
    /// the element.
    fn current(&mut self, a: Node, b: Node, lin_u: &[(usize, f64)]) {
        for (n, s) in [(a, -1.0), (b, 1.0)] {
            if let Node::Unknown(i) = n {
                for &(k, c) in lin_u {
                    self.bu.push((i, k, s * c));
                }
            }
        }
    }

    /// A voltage-controlled current `gm (v(cp) - v(cn))` into node `into` (from ground).
    fn vccs(&mut self, into: Node, cp: Node, cn: Node, gm: f64) {
        if let Node::Unknown(i) = into {
            for (n, s) in [(cp, gm), (cn, -gm)] {
                match n {
                    Node::Unknown(j) => self.g.push((i, j, -s)),
                    Node::Known(v) => self.bu.push((i, 0, s * v)),
                    Node::Input(k) => self.bu.push((i, k, s)),
                    Node::Ground => {}
                }
            }
        }
    }

    /// The injection pattern of a current from `a` to `b` through a device.
    fn inj(a: Node, b: Node) -> Vec<(usize, f64)> {
        let mut v = Vec::new();
        if let Node::Unknown(i) = a {
            v.push((i, -1.0));
        }
        if let Node::Unknown(i) = b {
            v.push((i, 1.0));
        }
        v
    }

    fn elem(
        &mut self,
        p: &Part,
        role: &'static str,
        dev: Dev,
        ctrl: Vec<Lin>,
        inj: Vec<Vec<(usize, f64)>>,
    ) {
        let (nv, ns, nq) = dev.shape();
        assert_eq!(ctrl.len(), nv, "{}: controls", p.refdes);
        assert_eq!(inj.len(), ns + nq, "{}: outputs", p.refdes);
        self.elems.push(Elem {
            refdes: p.refdes.clone(),
            section: p.section.clone(),
            role: Role(role),
            dev,
            pot: None,
            ctrl,
            inj,
        });
    }
}

fn bjt_model(library: &dv::Library, name: &str) -> &'static dv::Bjt {
    library
        .bjts
        .iter()
        .copied()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no transistor model {name}"))
}

fn diode_model(library: &dv::Library, name: &str) -> &'static dv::Diode {
    library
        .diodes
        .iter()
        .copied()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no diode model {name}"))
}

/// The element name a part gets in the netlists (`q_bd_q40`), for internal nodes' names.
fn el(p: &Part) -> String {
    let name: String = p
        .refdes
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .to_lowercase();
    format!("{}_{name}", p.section)
}

impl Net {
    /// The network of `parts` with `circuit`'s rails ideal and its device cards.
    pub fn build(parts: &[Part], opts: &Opts, circuit: &Circuit) -> Net {
        let mut b = Builder {
            names: Vec::new(),
            index: HashMap::new(),
            inputs: vec!["1".to_string()],
            input_index: HashMap::new(),
            rails: circuit.rails.iter().cloned().collect(),
            library: circuit.library,
            g: Vec::new(),
            c: Vec::new(),
            bu: Vec::new(),
            elems: Vec::new(),
        };
        for s in opts.sources.iter().chain(&opts.held) {
            let k = b.inputs.len();
            b.inputs.push(s.clone());
            b.input_index.insert(s.clone(), k);
        }
        let noise_input = opts.noise.as_ref().map(|_| {
            let k = b.inputs.len();
            b.inputs.push("noise".to_string());
            k
        });
        for p in parts {
            let nodes: Vec<Node> = p.nodes.iter().map(|n| b.node(n)).collect();
            let what = format!("{} {}", p.section, p.refdes);
            match &p.kind {
                Kind::R(r) | Kind::Link(r) => b.cond(nodes[0], nodes[1], 1.0 / r),
                Kind::Pot { r, taper, pos } if opts.live_pots => {
                    // Each section a conductance the knob sets in real time (D41); a section
                    // whose ends are one node, or one end of which is open, does nothing and is
                    // left out.
                    let (up, lo) = sim::pot_halves(*r, taper.frac(*pos));
                    for (a, c, g, upper, role) in [
                        (nodes[0], nodes[1], 1.0 / up, true, "pot 3-2"),
                        (nodes[1], nodes[2], 1.0 / lo, false, "pot 2-1"),
                    ] {
                        if a == c {
                            continue;
                        }
                        let ctrl = b.lin(a).minus(&b.lin(c));
                        let inj = Builder::inj(a, c);
                        b.elem(p, role, Dev::Cond(g), vec![ctrl], vec![inj]);
                        if let Some(e) = b.elems.last_mut() {
                            e.pot = Some((*r, *taper, upper));
                        }
                    }
                }
                Kind::Pot { r, taper, pos } => {
                    let (up, lo) = sim::pot_halves(*r, taper.frac(*pos));
                    b.cond(nodes[0], nodes[1], 1.0 / up);
                    b.cond(nodes[1], nodes[2], 1.0 / lo);
                }
                Kind::Trim { r, pos } => {
                    let (up, lo) = sim::pot_halves(*r, *pos);
                    b.cond(nodes[0], nodes[1], 1.0 / up);
                    b.cond(nodes[1], nodes[2], 1.0 / lo);
                }
                Kind::C(c) if !opts.cap_loss.is_none() => {
                    let (c0, branches) = sim::lossy_capacitor(*c, opts.cap_loss);
                    b.cap(nodes[0], nodes[1], c0, &what);
                    for (k, (r, ck)) in branches.iter().enumerate() {
                        let mid = Node::Unknown(b.internal(format!("c_{}_l{k}", el(p))));
                        b.cond(nodes[0], mid, 1.0 / r);
                        b.cap(mid, nodes[1], *ck, &what);
                    }
                }
                Kind::C(c) | Kind::Ce(c) | Kind::Cbp(c) => b.cap(nodes[0], nodes[1], *c, &what),
                Kind::Q(name) => {
                    let m = bjt_model(&b.library, name);
                    let (nc, nb, ne) = (nodes[0], nodes[1], nodes[2]);
                    if m.bv_eb.is_finite() {
                        // A transistor used for its junctions (the noise source's Q35): two
                        // diodes, as `models.rs` writes it.
                        let (b_an, e_cat) = if m.pnp { (ne, nb) } else { (nb, ne) };
                        let (bc_an, bc_cat) = if m.pnp { (nc, nb) } else { (nb, nc) };
                        let eb = dev::DiodeP::from_parts(
                            m.is,
                            1.0,
                            m.cje,
                            m.vje,
                            m.mje,
                            0.0,
                            m.bv_eb,
                            1e-5,
                            opts.reltol,
                        );
                        let bc = dev::DiodeP::from_parts(
                            m.is,
                            1.0,
                            m.cjc,
                            m.vjc,
                            m.mjc,
                            0.0,
                            f64::INFINITY,
                            0.0,
                            opts.reltol,
                        );
                        // The emitter junction's RS (re + rb) at its anode.
                        let ai = Node::Unknown(b.internal(format!("{}_eb_rs", el(p))));
                        b.cond(b_an, ai, 1.0 / (m.re + m.rb));
                        let mut ctrl = b.lin(ai).minus(&b.lin(e_cat));
                        let noisy = opts
                            .noise
                            .as_ref()
                            .is_some_and(|(s, r)| *s == p.section && *r == p.refdes);
                        if noisy {
                            // The cathode is the emitter behind the series noise voltage.
                            let k = noise_input.unwrap_or(0);
                            ctrl.u.push((k, if m.pnp { 1.0 } else { -1.0 }));
                        }
                        let inj = Builder::inj(ai, e_cat);
                        b.elem(p, "eb", Dev::Diode(eb), vec![ctrl], vec![inj.clone(), inj]);
                        let ctrl = b.lin(bc_an).minus(&b.lin(bc_cat));
                        let inj = Builder::inj(bc_an, bc_cat);
                        b.elem(p, "bc", Dev::Diode(bc), vec![ctrl], vec![inj.clone(), inj]);
                        continue;
                    }
                    let int = |b: &mut Builder, n: Node, r: f64, tag: &str| {
                        if r > 0.0 {
                            let i = Node::Unknown(b.internal(format!("q_{}#{tag}", el(p))));
                            b.cond(n, i, 1.0 / r);
                            i
                        } else {
                            n
                        }
                    };
                    let ci = int(&mut b, nc, m.rc, "collector");
                    let bi = int(&mut b, nb, m.rb, "base");
                    let ei = int(&mut b, ne, m.re, "emitter");
                    let s = if m.pnp { -1.0 } else { 1.0 };
                    let vbe = b.lin(bi).minus(&b.lin(ei)).scaled(s);
                    let vbc = b.lin(bi).minus(&b.lin(ci)).scaled(s);
                    // ic into the collector, ib into the base (times the polarity), the
                    // emitter returning both; the charges from the base to the emitter and to
                    // the collector.
                    let into = |n: Node, sign: f64| -> Vec<(usize, f64)> {
                        match n {
                            Node::Unknown(i) => vec![(i, sign)],
                            _ => vec![],
                        }
                    };
                    let mut ic = into(ci, -s);
                    ic.extend(into(ei, s));
                    let mut ib = into(bi, -s);
                    ib.extend(into(ei, s));
                    let mut qbe = into(bi, -s);
                    qbe.extend(into(ei, s));
                    let mut qbc = into(bi, -s);
                    qbc.extend(into(ci, s));
                    let model = match opts
                        .soft
                        .iter()
                        .find(|x| x.0 == p.section && x.1 == p.refdes)
                    {
                        // (n 0: straightened above i_ref instead.)
                        Some(&(_, _, 0.0, i_ref)) => dev::BjtP::straightened(m, i_ref),
                        Some(&(_, _, n, i_ref)) => dev::BjtP::soft(m, n, i_ref),
                        None => dev::BjtP::new(m),
                    };
                    b.elem(
                        p,
                        "",
                        Dev::Bjt(model),
                        vec![vbe, vbc],
                        vec![ic, ib, qbe, qbc],
                    );
                }
                Kind::D(name) => {
                    let m = diode_model(&b.library, name);
                    let (na, nk) = (nodes[0], nodes[1]);
                    let ai = if m.rs > 0.0 {
                        let i = Node::Unknown(b.internal(format!("d_{}#anode", el(p))));
                        b.cond(na, i, 1.0 / m.rs);
                        i
                    } else {
                        na
                    };
                    let ctrl = b.lin(ai).minus(&b.lin(nk));
                    let inj = Builder::inj(ai, nk);
                    b.elem(
                        p,
                        "",
                        Dev::Diode(dev::DiodeP::new(m, opts.reltol)),
                        vec![ctrl],
                        vec![inj.clone(), inj],
                    );
                }
                Kind::J(name) => {
                    let m = b
                        .library
                        .jfets
                        .iter()
                        .copied()
                        .find(|j| j.name == *name)
                        .unwrap_or_else(|| panic!("no JFET model {name}"));
                    let (mut nd, ng, mut ns) = (nodes[0], nodes[1], nodes[2]);
                    // The ohmic contacts, as ngspice adds them: internal drain and source nodes.
                    if m.rd > 0.0 {
                        let di = Node::Unknown(b.internal(format!("x_{}#drain", el(p))));
                        b.cond(nd, di, 1.0 / m.rd);
                        nd = di;
                    }
                    if m.rs > 0.0 {
                        let si = Node::Unknown(b.internal(format!("x_{}#source", el(p))));
                        b.cond(ns, si, 1.0 / m.rs);
                        ns = si;
                    }
                    let vgs = b.lin(ng).minus(&b.lin(ns));
                    let vgd = b.lin(ng).minus(&b.lin(nd));
                    let id = Builder::inj(nd, ns);
                    let gs = Builder::inj(ng, ns);
                    let gd = Builder::inj(ng, nd);
                    b.elem(
                        p,
                        "",
                        Dev::Jfet(dev::JfetP::new(m)),
                        vec![vgs, vgd],
                        vec![id, gs.clone(), gd.clone(), gs, gd],
                    );
                }
                Kind::M(name) => {
                    let m = b
                        .library
                        .mos
                        .iter()
                        .copied()
                        .find(|m| m.name == *name)
                        .unwrap_or_else(|| panic!("no MOS model {name}"));
                    let (nd, ng, ns, nb) = (nodes[0], nodes[1], nodes[2], nodes[3]);
                    let vgs = b.lin(ng).minus(&b.lin(ns));
                    let vds = b.lin(nd).minus(&b.lin(ns));
                    let vbs = b.lin(nb).minus(&b.lin(ns));
                    let vbd = b.lin(nb).minus(&b.lin(nd));
                    let id = Builder::inj(nd, ns);
                    let bs = Builder::inj(nb, ns);
                    let bd = Builder::inj(nb, nd);
                    b.elem(
                        p,
                        "",
                        Dev::Mos(dev::MosP::new(m)),
                        vec![vgs, vds, vbs, vbd],
                        vec![id, bs, bd],
                    );
                }
                Kind::OpAmp(name) => {
                    let o = b
                        .library
                        .opamps
                        .iter()
                        .copied()
                        .find(|m| m.name == *name)
                        .unwrap_or_else(|| panic!("no op-amp model {name}"));
                    let (np, nn, no, nvp, nvn) = (nodes[0], nodes[1], nodes[2], nodes[3], nodes[4]);
                    let x = Node::Unknown(b.internal(format!("x_{}#x", el(p))));
                    // The inputs' bias currents, out of each input.
                    b.current(np, Node::Ground, &[(0, o.ib)]);
                    b.current(nn, Node::Ground, &[(0, o.ib)]);
                    let input = dev::OpIn::new(o);
                    let rx = o.a0 / input.gm;
                    let cx = input.gm / (2.0 * std::f64::consts::PI * o.gbw);
                    b.cond(x, Node::Ground, 1.0 / rx);
                    b.cap(x, Node::Ground, cx, &what);
                    // The output: v(x) behind rout.
                    b.cond(no, Node::Ground, 1.0 / o.rout);
                    b.vccs(no, x, Node::Ground, 1.0 / o.rout);
                    let vd = b.lin(np).minus(&b.lin(nn));
                    let into_x = Builder::inj(Node::Ground, x);
                    b.elem(p, "in", Dev::OpIn(input), vec![vd], vec![into_x]);
                    let hi = b.lin(x).minus(&b.lin(nvp)).plus(&Lin::constant(o.vsat_pos));
                    let lo = b.lin(nvn).plus(&Lin::constant(o.vsat_neg)).minus(&b.lin(x));
                    b.elem(
                        p,
                        "clamp",
                        Dev::Clamp(dev::Clamp {
                            g: o.clamp_g,
                            knee: o.clamp_knee,
                        }),
                        vec![hi, lo],
                        vec![Builder::inj(x, Node::Ground)],
                    );
                }
                Kind::Schmitt(name) => {
                    let m = b
                        .library
                        .schmitts
                        .iter()
                        .copied()
                        .find(|m| m.name == *name)
                        .unwrap_or_else(|| panic!("no Schmitt model {name}"));
                    let (ni, no, nvdd, nvss) = (nodes[0], nodes[1], nodes[2], nodes[3]);
                    b.cap(ni, nvss, m.cin, &what);
                    let sp = dev::SchmittP::new(m);
                    let s = Node::Unknown(b.internal(format!("x_{}#s", el(p))));
                    b.cond(s, Node::Ground, 1.0 / sp.r);
                    b.cap(s, Node::Ground, sim::SCHMITT_CS, &what);
                    let vss = b.lin(nvss);
                    let ctrl = vec![
                        b.lin(ni).minus(&vss),
                        b.lin(nvdd).minus(&vss),
                        b.lin(s),
                        b.lin(no).minus(&vss),
                    ];
                    let inj = vec![
                        Builder::inj(Node::Ground, s),
                        Builder::inj(nvdd, no),
                        Builder::inj(no, nvss),
                    ];
                    b.elem(p, "", Dev::Schmitt(sp), ctrl, inj);
                }
                Kind::Comparator(name) => {
                    let m = b
                        .library
                        .comparators
                        .iter()
                        .copied()
                        .find(|m| m.name == *name)
                        .unwrap_or_else(|| panic!("no comparator model {name}"));
                    let (np, nn, no, nvcc, ngnd) =
                        (nodes[0], nodes[1], nodes[2], nodes[3], nodes[4]);
                    // Bias currents from VCC into each input.
                    b.current(nvcc, np, &[(0, m.ib)]);
                    b.current(nvcc, nn, &[(0, m.ib)]);
                    b.cap(np, Node::Ground, m.cin, &what);
                    b.cap(nn, Node::Ground, m.cin, &what);
                    b.cap(no, Node::Ground, m.cout, &what);
                    let cp = dev::CmpP::new(m);
                    let s = Node::Unknown(b.internal(format!("x_{}#s", el(p))));
                    b.cond(s, Node::Ground, 1.0 / cp.r);
                    b.cap(s, Node::Ground, sim::CMP_CS, &what);
                    let vcc = b.lin(nvcc);
                    let ctrl = vec![
                        b.lin(nn).minus(&b.lin(np)),
                        b.lin(np).minus(&vcc),
                        b.lin(nn).minus(&vcc),
                        b.lin(s),
                        b.lin(no).minus(&b.lin(ngnd)),
                    ];
                    let inj = vec![Builder::inj(Node::Ground, s), Builder::inj(no, ngnd)];
                    b.elem(p, "", Dev::Cmp(cp), ctrl, inj);
                }
                Kind::Ota(name) => {
                    let o = b
                        .library
                        .ota(name)
                        .unwrap_or_else(|| panic!("no OTA model {name}"));
                    let pin = |want: &str| {
                        o.pins
                            .iter()
                            .position(|p| *p == want)
                            .unwrap_or_else(|| panic!("{}: no pin {want}", o.name))
                    };
                    let (inp, inn, out) = (nodes[pin("in+")], nodes[pin("in-")], nodes[pin("out")]);
                    let (iabc, vn) = (nodes[pin("iabc")], nodes[pin("v-")]);
                    b.cond(out, Node::Ground, 1.0 / o.rout);
                    b.cap(out, Node::Ground, o.cout, &what);
                    let ctrl = vec![b.lin(iabc).minus(&b.lin(vn)), b.lin(inp).minus(&b.lin(inn))];
                    let inj = vec![Builder::inj(iabc, vn), Builder::inj(Node::Ground, out)];
                    b.elem(p, "", Dev::Ota(dev::OtaP::new(o)), ctrl, inj);
                    if o.pins.contains(&"v+") {
                        let vp = nodes[pin("v+")];
                        let hi = b
                            .lin(out)
                            .minus(&b.lin(vp))
                            .plus(&Lin::constant(sim::OTA_SAT));
                        let lo = b
                            .lin(vn)
                            .plus(&Lin::constant(sim::OTA_SAT))
                            .minus(&b.lin(out));
                        b.elem(
                            p,
                            "clamp",
                            Dev::Clamp(dev::Clamp {
                                g: sim::OPAMP_CLAMP_G,
                                knee: sim::OPAMP_CLAMP_KNEE,
                            }),
                            vec![hi, lo],
                            vec![Builder::inj(out, Node::Ground)],
                        );
                    }
                    if o.pins.contains(&"buf_in") && o.pins.contains(&"buf_out") {
                        let (bi, bo) = (nodes[pin("buf_in")], nodes[pin("buf_out")]);
                        b.cond(bo, Node::Ground, 1.0 / sim::OTA_BUF_R);
                        b.vccs(bo, bi, Node::Ground, 1.0 / sim::OTA_BUF_R);
                    }
                }
            }
        }
        // SPICE's rshunt on every node.
        let n = b.names.len();
        for i in 0..n {
            b.g.push((i, i, 1.0 / sim::RSHUNT));
        }
        let nu = b.inputs.len();
        let mut g = Mat::zeros(n, n);
        let mut c = Mat::zeros(n, n);
        let mut bu = Mat::zeros(n, nu);
        for &(i, j, v) in &b.g {
            g[(i, j)] += v;
        }
        for &(i, j, v) in &b.c {
            c[(i, j)] += v;
        }
        for &(i, j, v) in &b.bu {
            bu[(i, j)] += v;
        }
        Net {
            names: b.names,
            inputs: b.inputs,
            g,
            c,
            bu,
            elems: b.elems,
        }
    }

    pub fn n(&self) -> usize {
        self.names.len()
    }

    /// The unknown named `name`.
    pub fn node(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// The input named `name`.
    pub fn input(&self, name: &str) -> Option<usize> {
        self.inputs.iter().position(|n| n == name)
    }
}
