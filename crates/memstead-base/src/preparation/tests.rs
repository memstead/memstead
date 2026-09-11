#![cfg(test)]

use super::*;
use crate::entity::EntityId;
use indexmap::IndexMap;
use memstead_schema::types::{SectionDef, TypeDefinition};

fn section(key: &str, required: bool, load_bearing: Option<bool>) -> SectionDef {
    let mut v = serde_json::json!({
        "key": key, "heading": key, "required": required, "search_weight": 1.0
    });
    if let Some(lb) = load_bearing {
        v["load_bearing"] = serde_json::json!(lb);
    }
    serde_json::from_value(v).unwrap()
}

/// A real builtin type with its sections replaced — the fixture never
/// has to track `TypeDefinition`'s required-field roster.
fn type_with(sections: Vec<SectionDef>) -> TypeDefinition {
    let schemas = memstead_schema::builtins::load_builtin_schemas().unwrap();
    let base = schemas
        .iter()
        .find_map(|s| s.get_type("assertion"))
        .expect("a builtin schema declares `assertion`");
    let mut td = (*base).clone();
    td.sections = sections;
    td
}

fn entity(sections: &[(&str, &str)]) -> Entity {
    let mut map = IndexMap::new();
    for (k, v) in sections {
        map.insert(k.to_string(), v.to_string());
    }
    Entity {
        id: EntityId::canonical("m--e"),
        title: "E".into(),
        entity_type: "t".into(),
        mem: "m".into(),
        file_path: "e.md".into(),
        metadata: IndexMap::new(),
        sections: map,
        relationships: Vec::new(),
        content_hash: "h".into(),
        stub: false,
        stub_kind: None,
        heading_spans: Default::default(),
        raw_section_headings: Vec::new(),
    }
}

#[test]
fn registry_knows_its_four_flavours_and_nothing_else() {
    assert!(is_registered(ENTITY_LOAD_BEARING));
    assert!(is_registered(DATED_ENTRIES));
    assert!(is_registered(CODE_MAP));
    assert!(is_registered(QUOTED_PHRASE));
    assert!(!is_registered("pdf-to-markdown"));
    assert!(!is_registered(""));
    assert_eq!(
        registered_identifiers(),
        vec![ENTITY_LOAD_BEARING, DATED_ENTRIES, CODE_MAP, QUOTED_PHRASE]
    );
    let q = lookup(QUOTED_PHRASE).unwrap();
    assert_eq!(q.touchpoint, Touchpoint::PreparedForm);
    assert!(applies_to_namespace(q, "path"));
    assert!(applies_to_namespace(q, "path+commit"));
    assert!(applies_to_namespace(q, "entity"));
    assert!(applies_to_namespace(q, "url"));
    assert!(delivery_preparation(Some(QUOTED_PHRASE)).is_none());
    let c = lookup(CODE_MAP).unwrap();
    assert_eq!(c.touchpoint, Touchpoint::PreparedForm);
    assert!(applies_to_namespace(c, "path"));
    assert!(applies_to_namespace(c, "path+commit"));
    assert!(!applies_to_namespace(c, "entity"));
    assert!(!applies_to_namespace(c, "url"));
    assert!(delivery_preparation(Some(CODE_MAP)).is_none());
    let p = lookup(ENTITY_LOAD_BEARING).unwrap();
    assert_eq!(p.touchpoint, Touchpoint::PreparedForm);
    assert!(applies_to_namespace(p, "entity"));
    assert!(!applies_to_namespace(p, "path"));
    assert!(!applies_to_namespace(p, "url"));
    let d = lookup(DATED_ENTRIES).unwrap();
    assert_eq!(d.touchpoint, Touchpoint::DeliveryUnits);
    assert!(applies_to_namespace(d, "path"));
    assert!(applies_to_namespace(d, "path+commit"));
    assert!(!applies_to_namespace(d, "entity"));
    assert!(!applies_to_namespace(d, "url"));
    // Touchpoint B lookup: only a delivery flavour answers.
    assert_eq!(
        delivery_preparation(Some(DATED_ENTRIES)).map(|p| p.id),
        Some(DATED_ENTRIES)
    );
    assert!(delivery_preparation(Some(ENTITY_LOAD_BEARING)).is_none());
    assert!(delivery_preparation(Some("pdf-to-markdown")).is_none());
    assert!(delivery_preparation(None).is_none());
    assert!(unitize(ENTITY_LOAD_BEARING, "x").is_none());
    assert!(unitize("pdf-to-markdown", "x").is_none());
}

const LOG: &str = "# Ops log\n\nPreamble text.\n\n## 2026-08-24 10:05 boot\nline a\n\n\
                       - 2026-08-24T10:05:00Z boot again\nline b\n2026-08-25 shutdown\nline c\n";

/// Unitization: entries open at dated lines, the preamble folds into the
/// first unit, same-stamp entries get an ordinal, an undated file is one
/// `whole` unit, and the stamp normalizes across the accepted spellings.
#[test]
fn dated_entries_unitize_deterministically() {
    let units = unitize(DATED_ENTRIES, LOG).unwrap();
    let keys: Vec<&str> = units.iter().map(|u| u.key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "2026-08-24T10:05:00",
            "2026-08-24T10:05:00.2",
            "2026-08-25T00:00:00"
        ]
    );
    assert_eq!(
        units[0].start_line, 1,
        "the preamble folds into the first unit"
    );
    assert_eq!((units[0].end_line, units[1].start_line), (7, 8));
    assert_eq!(units[2].end_line, 11);
    assert_eq!(units[1].order_key, "2026-08-24T10:05:00");
    assert!(unit_text(LOG, &units[2]).starts_with("2026-08-25 shutdown"));
    assert_eq!(
        units[2].hash,
        prepared_content_hash(unit_text(LOG, &units[2]).as_bytes())
    );

    let whole = unitize(DATED_ENTRIES, "no stamps here\njust prose\n").unwrap();
    assert_eq!(whole.len(), 1);
    assert_eq!(whole[0].key, WHOLE_FILE_UNIT);
    assert_eq!(whole[0].order_key, "");

    assert_eq!(
        leading_stamp("[2026-02-30] bad day"),
        None,
        "day out of range"
    );
    assert_eq!(
        leading_stamp("2026-08-24T25:00 x"),
        None,
        "hour out of range"
    );
    assert_eq!(leading_stamp("v2026-08-24"), None, "not at the line start");
    assert_eq!(leading_stamp("2026-08-2400"), None, "digits run on");
    assert_eq!(
        leading_stamp("> **2026-08-24T10:05:00.250+02:00** note").as_deref(),
        Some("2026-08-24T10:05:00")
    );
    assert_eq!(
        unit_id("logs/ops.md", "2026-08-25T00:00:00"),
        "logs/ops.md#2026-08-25T00:00:00"
    );
    assert_eq!(
        split_unit_id("logs/ops.md#2026-08-25T00:00:00"),
        ("logs/ops.md", Some("2026-08-25T00:00:00"))
    );
    assert_eq!(split_unit_id("logs/ops.md"), ("logs/ops.md", None));
}

const JS: &str = "// Auth module\nimport axios from 'axios'\nimport { t } from '@/i18n'\n\n/* block\n   comment */\nconst RETRIES = 3\n\nexport default {\n  name: 'Auth',\n  props: ['user'],\n  data() {\n    return { token: null, busy: false }\n  },\n  methods: {\n    async login(user, password) {\n      // body\n      const r = await axios.post('/login', { user, password })\n      return r.data\n    },\n    logout() {\n      this.token = null\n    }\n  }\n}\n\nexport function helper(a, b) {\n  return a + b\n}\n\nexport const LIMIT = { max: 10 }\n";

/// The code map keeps the interface and nothing else: comments,
/// formatting and implementation bodies are invisible; a signature or
/// export change is visible; the digest is the same for JS and for the
/// script block of a Vue component.
#[test]
fn code_map_digest_sees_interfaces_not_bodies() {
    let digest = code_map_digest("src/auth.js", JS);
    assert_eq!(
        digest,
        "import axios from 'axios'\nimport{t}from '@/i18n'\nconst RETRIES=\n\
             export default\nname:\nprops:['user']\ndata()\nmethods:\n\
             async login(user,password)\nlogout()\nexport function helper(a,b)\n\
             export const LIMIT="
    );
    // A top-level value is body whatever its shape: a ternary or a
    // binary expression wrapped by a formatter, a member chain, an
    // object opened with `({`; a function-valued binding keeps its
    // signature, with or without parentheses around a lone parameter.
    let value_forms = [
        "export const base = cfg.API ? cfg.API : 'x'\n",
        "export const base = cfg.API\n  ? cfg.API\n  : 'x'\n",
        "export const base =\n  'a' +\n  'b'\n",
        "export const base = new Client({\n  region: 'eu',\n  retries: 3,\n})\n",
        "export const base = axios\n  .create(cfg)\n  .interceptors\n",
    ];
    let cut: Vec<String> = value_forms
        .iter()
        .map(|t| code_map_digest("cfg.js", t))
        .collect();
    assert!(cut.iter().all(|d| d == "export const base="), "{cut:?}");
    assert_eq!(
        code_map_digest(
            "s.js",
            "const store = new Vuex.Store({\n  state: { n: 1 },\n  mutations: {\n    inc(s) { s.n += 1 }\n  }\n})\n"
        ),
        code_map_digest(
            "s.js",
            "const store = new Vuex.Store({\n  state: { n: 1 },\n  mutations: {\n    inc(s) { s.n += 2 }\n  }\n})\n"
        )
    );
    assert_eq!(
        code_map_digest("d.js", "export default new Vuetify({\n  theme: 'x',\n})\n"),
        "export default"
    );
    assert_eq!(
        code_map_digest("f.js", "const f = x => x.id\n"),
        code_map_digest("f.js", "const f = (x) => x.id\n")
    );
    assert_eq!(
        code_map_digest("f.js", "const f = (x) => x.id\n"),
        "const f=x=>"
    );
    assert_eq!(
        code_map_digest(
            "f.js",
            "export const g = async (a, b) => {\n  return a\n}\n"
        ),
        "export const g=async(a,b)=>"
    );
    // Brace style and quoted keys are formatting.
    let knr = "class S {\n  login(user, password) {\n    return 1\n  }\n  logout() {\n  }\n}\n";
    let allman =
        "class S\n{\n  login(user, password)\n  {\n    return 1\n  }\n  logout()\n  {\n  }\n}\n";
    assert_eq!(
        code_map_digest("s.js", knr),
        "class S\nlogin(user,password)\nlogout()"
    );
    assert_eq!(
        code_map_digest("s.js", allman),
        code_map_digest("s.js", knr)
    );
    // Formatter line wrapping in every shape a formatter produces: an
    // arrow's expression body after `=>`, a property arrow with or
    // without parentheses, a call statement wrapped inside a body (which
    // never enters the digest), CommonJS `exports` values, a union type
    // led by `|`, rustfmt-wrapped generics.
    let same = |a: &str, b: &str, why: &str| {
        assert_eq!(
            code_map_digest("w.js", a),
            code_map_digest("w.js", b),
            "{why}"
        );
    };
    same(
        "export const pick = state => state.items.filter(i => i.active).map(i => i.id)\n",
        "export const pick = state =>\n  state.items\n    .filter(i => i.active)\n    .map(i => i.id)\n",
        "arrow expression body wrapped",
    );
    assert_eq!(
        code_map_digest("w.js", "export const pick = (state) => state.items\n"),
        "export const pick=state=>"
    );
    same(
        "export default {\n  select: state => state.items.filter(i => i.active),\n}\n",
        "export default {\n  select: state =>\n    state.items.filter(i => i.active),\n}\n",
        "property arrow body wrapped",
    );
    same(
        "module.exports = {\n  validate: (v) => {\n    return v\n  },\n}\n",
        "module.exports = {\n  validate: v => {\n    return v\n  },\n}\n",
        "arrowParens on a property arrow",
    );
    assert_eq!(
        code_map_digest(
            "w.js",
            "module.exports = {\n  validate: v => {\n    return v\n  },\n}\n"
        ),
        "module.exports=\nvalidate:v=>"
    );
    same(
        "export function setup(app) {\n  registerPlugin(app, options, extra)\n}\n",
        "export function setup(app) {\n  registerPlugin(\n    app,\n    options,\n    extra\n  )\n}\n",
        "wrapped call statement in a function body",
    );
    assert_eq!(
        code_map_digest(
            "w.js",
            "export function setup(app) {\n  registerPlugin(\n    app,\n    options,\n    extra\n  )\n}\n"
        ),
        "export function setup(app)"
    );
    same(
        "class S {\n  run() {\n    helper(a, b, c)\n  }\n}\n",
        "class S {\n  run() {\n    helper(\n      a,\n      b,\n      c\n    )\n  }\n}\n",
        "wrapped call in a class method body",
    );
    assert_eq!(
        code_map_digest(
            "s.rs",
            "impl S {\n    pub fn run(&self) {\n        helper(\n            a,\n            b,\n        )\n    }\n}\n"
        ),
        code_map_digest(
            "s.rs",
            "impl S {\n    pub fn run(&self) {\n        helper(a, b)\n    }\n}\n"
        )
    );
    same(
        "exports.base = cfg.API ? cfg.API : 'http://localhost'\n",
        "exports.base = cfg.API\n  ? cfg.API\n  : 'http://localhost'\n",
        "exports ternary wrapped",
    );
    same(
        "module.exports = mongoose.model('User', schema).plugin(paginate)\n",
        "module.exports = mongoose\n  .model('User', schema)\n  .plugin(paginate)\n",
        "module.exports chain wrapped",
    );
    assert_eq!(
        code_map_digest(
            "w.js",
            "exports.TIMEOUT = compute(\n  settings,\n  defaults\n)\n"
        ),
        "exports.TIMEOUT="
    );
    assert_eq!(
        code_map_digest(
            "t.ts",
            "export type Mode = 'discovery' | 'sync' | 'verify'\n"
        ),
        code_map_digest(
            "t.ts",
            "export type Mode =\n  | 'discovery'\n  | 'sync'\n  | 'verify'\n"
        )
    );
    assert_ne!(
        code_map_digest("t.ts", "export type Mode = 'discovery' | 'sync'\n"),
        code_map_digest(
            "t.ts",
            "export type Mode = 'discovery' | 'sync' | 'verify'\n"
        ),
        "a union member is interface"
    );
    assert_eq!(
        code_map_digest(
            "g.rs",
            "pub fn all(&self) -> Result<Vec<String>, Error> {\n    todo!()\n}\n"
        ),
        code_map_digest(
            "g.rs",
            "pub fn all(\n    &self,\n) -> Result<\n    Vec<String>,\n    Error,\n> {\n    todo!()\n}\n"
        )
    );
    // rustfmt and prettier breaking a value or a type onto the next line
    // (`pub const`, a struct field's type, a type alias, a class field, a
    // bare object key); callback bodies never enter the digest; a
    // bracket-opened value is skipped whole.
    assert_eq!(
        code_map_digest(
            "c.rs",
            "pub const DESCRIPTION: &str =\n    \"a long description\";\n"
        ),
        code_map_digest(
            "c.rs",
            "pub const DESCRIPTION: &str = \"a long description\";\n"
        )
    );
    assert_eq!(
        code_map_digest("c.rs", "pub const DESCRIPTION: &str = \"x\";\n"),
        "pub const DESCRIPTION:&str="
    );
    assert_eq!(
        code_map_digest(
            "f.rs",
            "pub struct H {\n    pub handler:\n        Box<dyn Fn(&str) -> Result<(), Error> + Send>,\n}\n"
        ),
        code_map_digest(
            "f.rs",
            "pub struct H {\n    pub handler: Box<dyn Fn(&str) -> Result<(), Error> + Send>,\n}\n"
        )
    );
    assert_eq!(
        code_map_digest(
            "t.rs",
            "pub type Handler =\n    Box<dyn Fn(&str) -> Result<(), Error>>;\n"
        ),
        code_map_digest(
            "t.rs",
            "pub type Handler = Box<dyn Fn(&str) -> Result<(), Error>>;\n"
        )
    );
    assert!(code_map_digest("t.rs", "pub type Handler =\n    Box<X>;\n").contains("Box<X>"));
    same(
        "class Api {\n  static url = 'a' + 'b';\n  private readonly base = x || 'y';\n}\n",
        "class Api {\n  static url =\n    'a' +\n    'b';\n  private readonly base =\n    x || 'y';\n}\n",
        "class fields wrapped after =",
    );
    assert_eq!(
        code_map_digest("w.js", "class Api {\n  static url = 'a';\n}\n"),
        "class Api\nstatic url="
    );
    same(
        "export default {\n  message: 'a' + 'b',\n  data() {\n    return {}\n  },\n}\n",
        "export default {\n  message:\n    'a' +\n    'b',\n  data() {\n    return {}\n  },\n}\n",
        "bare key wrapped away from its value",
    );
    assert!(
        code_map_digest(
            "w.js",
            "export default {\n  message:\n    'a' +\n    'b',\n}\n"
        )
        .contains("message:")
    );
    same(
        "it('logs in', async () => {\n  const r = await login()\n  expect(r).toBe(1)\n})\n",
        "it('logs in', async () => {\n  const r = await login();\n  expect(r).toBe(2);\n});\n",
        "a callback body is body",
    );
    same(
        "export function setup(app) {\n  setTimeout(() => {\n    app.start(1)\n  }, 10)\n}\n",
        "export function setup(app) {\n  setTimeout(() => {\n    app.start(2)\n  }, 10)\n}\n",
        "a callback body inside a function body",
    );
    same(
        "export default {\n  created() {\n    setTimeout(() => {\n      this.a = 1\n    }, 5)\n  },\n}\n",
        "export default {\n  created() {\n    setTimeout(() => {\n      this.a = 2\n    }, 5)\n  },\n}\n",
        "a callback body inside a member body",
    );
    assert_eq!(
        code_map_digest(
            "r.js",
            "export const routes = [\n  { path: '/', meta: { auth: true } },\n  { path: '/x' },\n]\n"
        ),
        "export const routes="
    );
    // Interface and enum members and destructured names are interface.
    let api = "export interface Api {\n  name: string\n  load(id: string): Promise<void>\n}\n";
    assert_eq!(
        code_map_digest("a.ts", api),
        "export interface Api\nname:string\nload(id:string):Promise<void>"
    );
    assert_ne!(
        code_map_digest("a.ts", api),
        code_map_digest("a.ts", &api.replace("name: string", "name: number"))
    );
    assert_ne!(
        code_map_digest("a.ts", api),
        code_map_digest(
            "a.ts",
            &api.replace("load(id: string)", "load(id: string, force: boolean)")
        )
    );
    let color = "export enum Color {\n  Red,\n  Green = 2,\n}\n";
    assert_eq!(
        code_map_digest("e.ts", color),
        "export enum Color\nRed\nGreen=2"
    );
    assert_ne!(
        code_map_digest("e.ts", color),
        code_map_digest("e.ts", &color.replace("Green = 2,", "Green = 2,\n  Blue,"))
    );
    assert_eq!(
        code_map_digest("q.js", "const { a, b } = require('./x')\n"),
        "const{a,b}="
    );
    assert_ne!(
        code_map_digest("q.js", "const { a, b } = require('./x')\n"),
        code_map_digest("q.js", "const { a, c } = require('./x')\n")
    );
    // A formatter's wrap of a typed member and of a destructuring pattern
    // digests as the one-line form, and an edit inside the wrap is seen.
    let wrapped_api = "export interface Api {\n  name: string\n  load(\n    id: string,\n    force: boolean,\n  ): Promise<void>\n}\n";
    assert_eq!(
        code_map_digest("a.ts", wrapped_api),
        code_map_digest(
            "a.ts",
            "export interface Api {\n  name: string\n  load(id: string, force: boolean): Promise<void>\n}\n"
        )
    );
    assert_ne!(
        code_map_digest("a.ts", wrapped_api),
        code_map_digest(
            "a.ts",
            &wrapped_api.replace(
                "force: boolean,\n",
                "force: boolean,\n    options: LoadOptions,\n"
            )
        )
    );
    let wrapped_require = "const {\n  a,\n  b,\n} = require('./x')\n";
    assert_eq!(code_map_digest("q.js", wrapped_require), "const{a,b}=");
    assert_ne!(
        code_map_digest("q.js", wrapped_require),
        code_map_digest("q.js", &wrapped_require.replace("  b,\n", "  c,\n"))
    );
    assert_eq!(
        code_map_digest("q.js", "export const [\n  first,\n  second,\n] = pair()\n"),
        "export const[first,second]="
    );
    assert_eq!(
        code_map_digest(
            "o.js",
            "export default {\n  'name': 'X',\n  props: ['a'],\n}\n"
        ),
        code_map_digest(
            "o.js",
            "export default {\n  name: 'X',\n  props: ['a'],\n}\n"
        )
    );
    let h = |text: &str| prepared_content_hash(code_map_digest("src/auth.js", text).as_bytes());
    let base = h(JS);
    // Comment, formatting, body: invisible.
    assert_eq!(h(&JS.replace("// body", "// rewritten comment")), base);
    assert_eq!(h(&JS.replace("  return a + b", "    return   a+b")), base);
    assert_eq!(h(&JS.replace("/login", "/session")), base);
    assert_eq!(h(&JS.replace("return r.data", "return r.data.user")), base);
    assert_eq!(
        h(&JS.replace("max: 10", "max: 20")),
        base,
        "a value is body"
    );
    // Formatting inside a declaration: invisible (comma spacing, a
    // wrapped signature, semicolons, quote style, a comment in the
    // parameter list).
    assert_eq!(
        h(&JS.replace("login(user, password)", "login(user,password)")),
        base
    );
    assert_eq!(
        h(&JS.replace(
            "login(user, password)",
            "login(\n      user,\n      password\n    )"
        )),
        base
    );
    assert_eq!(
        h(&JS.replace("import axios from 'axios'", "import axios from \"axios\";")),
        base
    );
    assert_eq!(
        h(&JS.replace("helper(a, b)", "helper (a /* first */, b)")),
        base
    );
    assert_eq!(
        h(&JS.replace("export const LIMIT = {", "export const LIMIT={")),
        base
    );
    // Formatter-class rewrites: a brace-wrapped import list, a trailing
    // comma on the last member, a wrapped array, a wrapped parameter
    // list with a trailing comma; and a scalar value is body in every form.
    assert_eq!(
        h(&JS.replace(
            "import { t } from '@/i18n'",
            "import {\n  t,\n} from '@/i18n'"
        )),
        base
    );
    assert_eq!(h(&JS.replace("props: ['user'],", "props: ['user']")), base);
    assert_eq!(
        h(&JS.replace("props: ['user'],", "props: [\n    'user',\n  ],")),
        base
    );
    assert_eq!(
        h(&JS.replace(
            "login(user, password)",
            "login(\n      user,\n      password,\n    )"
        )),
        base
    );
    assert_eq!(
        h(&JS.replace("name: 'Auth',", "name: 'Login',")),
        base,
        "a scalar value is body"
    );
    // A member added inside a wrapped import list is visible.
    assert_ne!(
        h(&JS.replace(
            "import { t } from '@/i18n'",
            "import {\n  t,\n  n,\n} from '@/i18n'"
        )),
        base
    );
    assert_eq!(
        code_map_digest("x.js", "export {\n  a,\n  b,\n} from './x'\n"),
        code_map_digest("x.js", "export { a, b } from './x'\n")
    );
    // Signature, export, import: visible.
    assert_ne!(
        h(&JS.replace("login(user, password)", "login(user, password, remember)")),
        base
    );
    assert_ne!(
        h(&JS.replace("export function helper", "function helper")),
        base
    );
    assert_ne!(h(&JS.replace("import axios from 'axios'\n", "")), base);
    assert_ne!(
        h(&JS.replace("props: ['user']", "props: ['user', 'tenant']")),
        base
    );
    // The same script inside a Vue component digests identically; the
    // template and style are not interface.
    let vue = format!(
        "<template>\n  <div @click=\"login\">{{{{ t('hi') }}}}</div>\n</template>\n\n<script>\n{JS}</script>\n\n<style scoped>\n.a {{ color: red }}\n</style>\n"
    );
    assert_eq!(code_map_digest("src/Auth.vue", &vue), digest);
    assert_eq!(
        code_map_digest("src/Auth.vue", &vue.replace("color: red", "color: blue")),
        digest
    );
    // Non-code files are taken whole; JSON canonicalizes formatting away.
    assert_eq!(
        code_map_digest("README.md", "# hi\n\ntext\n"),
        "# hi\n\ntext\n"
    );
    assert_eq!(
        code_map_digest(
            "package.json",
            "{\n  \"name\": \"x\",\n  \"version\": \"1\"\n}\n"
        ),
        code_map_digest("package.json", "{\"name\":\"x\",\"version\":\"1\"}")
    );
}

const PY: &str = "# -*- coding: utf-8 -*-\nimport os\nfrom typing import List\n\nTIMEOUT = 30  # seconds\n\n\
                      def load(path: str, *, strict: bool = False) -> List[str]:\n    \"\"\"Docstring.\"\"\"\n    with open(path) as f:\n        return f.readlines()\n\n\
                      class Loader:\n    retries = 3\n\n    @property\n    def name(self):\n        return 'x'\n\n    def run(self,\n            arg):\n        def inner():\n            pass\n        return arg\n";

#[test]
fn code_map_digest_python_and_rust() {
    assert_eq!(
        code_map_digest("pivot.py", PY),
        "import os\nfrom typing import List\nTIMEOUT=\n\
             def load(path:str,*,strict:bool=False)->List[str]\nclass Loader\n\
             @property\ndef name(self)\ndef run(self,arg)"
    );
    assert_eq!(
        code_map_digest(
            "pivot.py",
            &PY.replace(
                "def run(self,\n            arg):",
                "def run(\n        self,\n        arg,\n    ):"
            )
        ),
        code_map_digest("pivot.py", PY),
        "a formatter's trailing comma in a wrapped def is invisible"
    );
    assert_eq!(
        code_map_digest("i.py", "from typing import (\n    Dict,\n    List,\n)\n"),
        code_map_digest("i.py", "from typing import Dict, List\n"),
        "black's parenthesized import list is formatting"
    );
    let h = |t: &str| prepared_content_hash(code_map_digest("pivot.py", t).as_bytes());
    assert_eq!(
        h(PY),
        h(&PY.replace("return f.readlines()", "return list(f)"))
    );
    assert_eq!(h(PY), h(&PY.replace("Docstring.", "Another docstring.")));
    assert_ne!(
        h(PY),
        h(&PY.replace("def run(self,", "def run(self, extra,"))
    );

    let rs = "//! Module docs\nuse std::fmt;\n\n/// A thing.\n#[derive(Debug)]\npub struct Thing {\n    pub id: u32,\n    secret: String,\n}\n\nimpl Thing {\n    pub fn new(id: u32) -> Self {\n        Self { id, secret: String::new() }\n    }\n    fn hidden(&self) {}\n}\n";
    assert_eq!(
        code_map_digest("src/thing.rs", rs),
        "use std::fmt\n#[derive(Debug)]\npub struct Thing\npub id:u32\nimpl Thing\n\
             pub fn new(id:u32)->Self\nfn hidden(&self)"
    );
}

/// The plain tree digest is order-insensitive on input, sorted on
/// output, and moves on any byte change and on any file joining or
/// leaving — the whole-content posture of a `file` anchor lifted to the
/// directory, so a plain `tree` anchor adjudicates deterministically.
#[test]
fn plain_tree_digest_is_sorted_and_content_sensitive() {
    let files = vec![
        ("src/b.rs".to_string(), b"fn b() {}\n".to_vec()),
        ("src/a.rs".to_string(), b"fn a() {}\n".to_vec()),
    ];
    let base = plain_tree_digest(&files);
    assert!(
        base.starts_with(&format!(
            "{}  src/a.rs\n",
            prepared_content_hash(b"fn a() {}\n")
        )),
        "rows sort by path regardless of input order"
    );
    let reordered = vec![files[1].clone(), files[0].clone()];
    assert_eq!(plain_tree_digest(&reordered), base);
    let body_edit = vec![
        files[0].clone(),
        ("src/a.rs".to_string(), b"fn a() { /* edit */ }\n".to_vec()),
    ];
    assert_ne!(
        plain_tree_digest(&body_edit),
        base,
        "any byte change moves the plain digest (unlike the code map)"
    );
    let mut joined = files.clone();
    joined.push(("src/c.rs".to_string(), b"fn c() {}\n".to_vec()));
    assert_ne!(plain_tree_digest(&joined), base, "a joining file moves it");
    let left = vec![files[0].clone()];
    assert_ne!(plain_tree_digest(&left), base, "a leaving file moves it");
}

/// A tree's map changes when a file joins, leaves, or changes its
/// interface, and holds when only a body changes; the observation rule
/// routes each grain to its prepared form.
#[test]
fn code_map_tree_digest_and_path_rule() {
    let files = vec![
        ("src/b.js".to_string(), "export const B = 1\n".to_string()),
        ("src/a.js".to_string(), JS.to_string()),
    ];
    let base = code_map_tree_digest(&files);
    assert!(base.starts_with(&format!(
        "{}  src/a.js\n",
        prepared_content_hash(code_map_digest("src/a.js", JS).as_bytes())
    )));
    let body_edit = vec![
        files[0].clone(),
        ("src/a.js".to_string(), JS.replace("/login", "/session")),
    ];
    assert_eq!(
        code_map_tree_digest(&body_edit),
        base,
        "a body edit leaves the tree map"
    );
    let sig_edit = vec![
        files[0].clone(),
        (
            "src/a.js".to_string(),
            JS.replace("logout()", "logout(everywhere)"),
        ),
    ];
    assert_ne!(code_map_tree_digest(&sig_edit), base);
    let mut joined = files.clone();
    joined.push(("src/c.js".to_string(), "export const C = 1\n".to_string()));
    assert_ne!(code_map_tree_digest(&joined), base);

    let digest_hash = prepared_content_hash(code_map_digest("src/a.js", JS).as_bytes());
    assert_eq!(
        path_prepared_hash(Some(CODE_MAP), "src/a.js", AnchorGrain::File, JS.as_bytes()),
        PathPrepared::Hash(digest_hash.clone())
    );
    assert_eq!(
        path_prepared_hash(
            Some(CODE_MAP),
            "src/a.js#L1-L3",
            AnchorGrain::Span,
            JS.as_bytes()
        ),
        PathPrepared::Hash(digest_hash)
    );
    assert_eq!(
        path_prepared_hash(None, "src/a.js", AnchorGrain::File, JS.as_bytes()),
        PathPrepared::Hash(prepared_content_hash(JS.as_bytes())),
        "no preparation: the bytes, byte-for-byte as before"
    );
    assert_eq!(
        path_prepared_hash(Some(CODE_MAP), "src", AnchorGrain::Tree, b""),
        PathPrepared::NoHash,
        "a tree needs enumeration; the caller supplies it"
    );
    assert_eq!(
        path_prepared_hash(None, "src", AnchorGrain::Tree, b""),
        PathPrepared::NoHash
    );
    let log = "2026-08-24 one\nbody\n2026-08-25 two\nbody\n";
    assert!(matches!(
        path_prepared_hash(
            Some(DATED_ENTRIES),
            "log.md#2026-08-25T00:00:00",
            AnchorGrain::Span,
            log.as_bytes()
        ),
        PathPrepared::Hash(_)
    ));
    assert_eq!(
        path_prepared_hash(
            Some(DATED_ENTRIES),
            "log.md#2026-08-26T00:00:00",
            AnchorGrain::Span,
            log.as_bytes()
        ),
        PathPrepared::UnitAbsent
    );
    assert_eq!(
        path_prepared_hash(
            Some(DATED_ENTRIES),
            "log.md",
            AnchorGrain::File,
            log.as_bytes()
        ),
        PathPrepared::Hash(prepared_content_hash(log.as_bytes()))
    );
}

/// Keys are stable under growth: appending entries leaves every existing
/// unit's key and hash untouched, so a change run delivers only the new
/// unit; an edited entry delivers as modified, a removed one as deleted.
#[test]
fn unit_keys_survive_growth_and_diff_delivers_only_what_changed() {
    let before = unitize(DATED_ENTRIES, LOG).unwrap();
    let grown = format!("{LOG}2026-08-26 09:00 restart\nline d\n");
    let after = unitize(DATED_ENTRIES, &grown).unwrap();
    assert_eq!(
        &after[..3],
        &before[..],
        "existing units are byte-identical"
    );
    let delta = diff_units(&before, &after);
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0].0.key, "2026-08-26T09:00:00");
    assert_eq!(delta[0].1, UnitChange::Added);

    let edited = LOG.replace("line c", "line c, revised");
    let delta = diff_units(&before, &unitize(DATED_ENTRIES, &edited).unwrap());
    assert_eq!(
        delta
            .iter()
            .map(|(u, c)| (u.key.as_str(), *c))
            .collect::<Vec<_>>(),
        vec![("2026-08-25T00:00:00", UnitChange::Modified)]
    );

    let shrunk = LOG.replace("2026-08-25 shutdown\nline c\n", "");
    let delta = diff_units(&before, &unitize(DATED_ENTRIES, &shrunk).unwrap());
    assert_eq!(
        delta
            .iter()
            .map(|(u, c)| (u.key.as_str(), *c))
            .collect::<Vec<_>>(),
        vec![("2026-08-25T00:00:00", UnitChange::Deleted)]
    );
    assert!(diff_units(&before, &before).is_empty());
}

#[test]
fn url_defaults_unstable_every_other_grain_stable() {
    assert_eq!(
        default_hash_stability(AnchorGrain::Url),
        AnchorHashStability::Unstable
    );
    for g in [
        AnchorGrain::Span,
        AnchorGrain::File,
        AnchorGrain::Tree,
        AnchorGrain::Entity,
    ] {
        assert_eq!(default_hash_stability(g), AnchorHashStability::Stable);
    }
}

/// The url grain's prepared form IS the path grains' canonicalization:
/// same bytes, same hash, and the same noise (CRLF, BOM, final newline)
/// is invisible.
#[test]
fn url_prepared_form_is_the_shared_canonicalization() {
    let a = url_prepared_hash(b"<p>hello</p>\n");
    assert_eq!(a, prepared_content_hash(b"<p>hello</p>\n"));
    assert_eq!(a, url_prepared_hash(b"\xEF\xBB\xBF<p>hello</p>\r\n\r\n"));
    assert_ne!(a, url_prepared_hash(b"<p>hello!</p>\n"));
    assert_eq!(
        supplied_content_hash(AnchorGrain::Url, b"<p>hello</p>").as_deref(),
        Some(a.as_str())
    );
    assert!(supplied_content_hash(AnchorGrain::File, b"x").is_some());
    assert!(supplied_content_hash(AnchorGrain::Span, b"x").is_some());
    assert!(supplied_content_hash(AnchorGrain::Tree, b"x").is_none());
    assert!(supplied_content_hash(AnchorGrain::Entity, b"x").is_none());
}

#[test]
fn load_bearing_resolves_explicit_then_required_then_all() {
    let explicit = type_with(vec![
        section("claim", true, Some(true)),
        section("evidence", true, Some(false)),
        section("notes", false, None),
    ]);
    let keys: Vec<_> = load_bearing_sections(&explicit)
        .iter()
        .map(|s| s.key.as_str())
        .collect();
    assert_eq!(keys, vec!["claim"]);

    let required = type_with(vec![
        section("claim", true, None),
        section("evidence", true, Some(false)),
        section("notes", false, None),
    ]);
    let keys: Vec<_> = load_bearing_sections(&required)
        .iter()
        .map(|s| s.key.as_str())
        .collect();
    assert_eq!(
        keys,
        vec!["claim"],
        "a required section opted out is excluded"
    );

    let none = type_with(vec![section("a", false, None), section("b", false, None)]);
    let keys: Vec<_> = load_bearing_sections(&none)
        .iter()
        .map(|s| s.key.as_str())
        .collect();
    assert_eq!(keys, vec!["a", "b"], "no declaration at all: every section");
}

/// The anker metric, mechanised: a notes-only edit leaves the prepared
/// hash intact; a load-bearing edit breaks it.
#[test]
fn notes_edit_keeps_the_hash_load_bearing_edit_breaks_it() {
    let td = type_with(vec![
        section("decision", true, None),
        section("notes", false, None),
    ]);
    let base = entity(&[("decision", "We ship."), ("notes", "first draft")]);
    let notes_edit = entity(&[("decision", "We ship."), ("notes", "first draft, revised")]);
    let claim_edit = entity(&[("decision", "We do not ship."), ("notes", "first draft")]);
    let h = |e: &Entity| entity_prepared_hash(e, Some(&td), Some(ENTITY_LOAD_BEARING)).unwrap();
    assert_eq!(h(&base), h(&notes_edit));
    assert_ne!(h(&base), h(&claim_edit));

    // The default form (no preparation) sees BOTH edits — today's
    // behaviour, byte-for-byte the canonical rendered markdown.
    let d = |e: &Entity| entity_prepared_hash(e, Some(&td), None).unwrap();
    assert_ne!(d(&base), d(&notes_edit));
    assert_eq!(
        d(&base),
        prepared_content_hash(crate::render::render_entity_markdown(&base, None).as_bytes())
    );

    // An unregistered identifier computes nothing.
    assert!(entity_prepared_hash(&base, Some(&td), Some("pdf-to-markdown")).is_none());
}

/// Content moving between two load-bearing sections changes the form
/// (keys are part of it); trailing whitespace inside a section does not.
#[test]
fn form_is_keyed_and_trimmed() {
    let td = type_with(vec![
        section("claim", true, None),
        section("evidence", true, None),
    ]);
    let a = entity(&[("claim", "x"), ("evidence", "y")]);
    let b = entity(&[("claim", "y"), ("evidence", "x")]);
    let c = entity(&[("claim", "x  \n\n"), ("evidence", "\n y")]);
    let form = |e: &Entity| entity_load_bearing_form(e, Some(&td));
    assert_ne!(form(&a), form(&b));
    assert_eq!(form(&a), form(&c));
    assert_eq!(form(&a), "## claim\n\nx\n\n## evidence\n\ny\n\n");
    // No type definition: every section the entity carries, its order.
    assert_eq!(
        entity_load_bearing_form(&entity(&[("z", "1"), ("a", "2")]), None),
        "## z\n\n1\n\n## a\n\n2\n\n"
    );
}

#[test]
fn quoted_phrase_resolves_while_the_words_stand_and_is_absent_once_they_leave() {
    let text = "# Sizing\n\nA mem holds 1,000\u{2013}5,000 entities by design.\n";
    let present = path_prepared_hash(
        Some(QUOTED_PHRASE),
        "GLOSSARY.md#1,000\u{2013}5,000 entities by design",
        AnchorGrain::Span,
        text.as_bytes(),
    );
    let PathPrepared::Hash(h) = present else {
        panic!("a phrase the text carries prepares to a hash, got {present:?}");
    };
    // The hash is the phrase's own, so it holds under every rewrite that
    // keeps the words: a new paragraph around them changes nothing.
    let rewritten = "Preface.\n\nA mem holds 1,000\u{2013}5,000 entities by design, we say.\n";
    assert_eq!(
        path_prepared_hash(
            Some(QUOTED_PHRASE),
            "GLOSSARY.md#1,000\u{2013}5,000 entities by design",
            AnchorGrain::Span,
            rewritten.as_bytes(),
        ),
        PathPrepared::Hash(h.clone())
    );
    // CRLF and a BOM never decide a citation.
    let crlf = "\u{feff}A mem holds 1,000\u{2013}5,000\r\nentities by design.\r\n";
    assert_eq!(
        path_prepared_hash(
            Some(QUOTED_PHRASE),
            "GLOSSARY.md#1,000\u{2013}5,000\nentities by design",
            AnchorGrain::Span,
            crlf.as_bytes(),
        ),
        PathPrepared::Hash(prepared_content_hash(
            "1,000\u{2013}5,000\nentities by design".as_bytes()
        ))
    );
    // The words gone: the unit is absent, not a differing hash.
    assert_eq!(
        path_prepared_hash(
            Some(QUOTED_PHRASE),
            "GLOSSARY.md#1,000\u{2013}5,000 entities by design",
            AnchorGrain::Span,
            b"A mem holds typically 1,000 entities.\n",
        ),
        PathPrepared::UnitAbsent
    );
    // An empty phrase addresses nothing.
    assert_eq!(quoted_phrase_prepared("  ", text), PathPrepared::UnitAbsent);
    // The url grain prepares observation-supplied content the same way.
    assert_eq!(
        path_prepared_hash(
            Some(QUOTED_PHRASE),
            "https://example.test/llms.txt#by design",
            AnchorGrain::Url,
            text.as_bytes(),
        ),
        PathPrepared::Hash(prepared_content_hash(b"by design"))
    );
    // Without a locator the grain keeps its whole-text form.
    assert_eq!(
        path_prepared_hash(
            Some(QUOTED_PHRASE),
            "GLOSSARY.md",
            AnchorGrain::Span,
            text.as_bytes()
        ),
        PathPrepared::Hash(prepared_content_hash(text.as_bytes()))
    );
    assert_eq!(
        path_prepared_hash(
            Some(QUOTED_PHRASE),
            "https://example.test/",
            AnchorGrain::Url,
            text.as_bytes()
        ),
        PathPrepared::NoHash
    );
    // Other preparations are untouched by a phrase-shaped locator.
    assert_eq!(
        path_prepared_hash(
            None,
            "GLOSSARY.md#by design",
            AnchorGrain::Span,
            text.as_bytes()
        ),
        PathPrepared::Hash(prepared_content_hash(text.as_bytes()))
    );
}

#[test]
fn quoted_phrase_on_an_entity_reads_the_canonical_markdown() {
    let e = entity(&[("claim", "The store is the exact layer."), ("notes", "n")]);
    assert_eq!(
        entity_prepared(&e, None, Some(QUOTED_PHRASE), Some("the exact layer")),
        PathPrepared::Hash(prepared_content_hash(b"the exact layer"))
    );
    assert_eq!(
        entity_prepared(&e, None, Some(QUOTED_PHRASE), Some("the semantic layer")),
        PathPrepared::UnitAbsent
    );
    // No locator: the whole rendered form, as with no preparation.
    assert_eq!(
        entity_prepared(&e, None, Some(QUOTED_PHRASE), None),
        entity_prepared(&e, None, None, None)
    );
    // The thin wrapper still answers for the older flavours and
    // reports an unknown identifier as unprepared.
    assert!(entity_prepared_hash(&e, None, None).is_some());
    assert!(entity_prepared_hash(&e, None, Some("pdf-to-markdown")).is_none());
}
