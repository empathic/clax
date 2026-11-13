//! ClaxBot surface definitions
//! (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`, "The
//! surface file"): what a host, or Clax itself, gives a run, as data.
//!
//! A [`Definition`] is one surface: its name and rules, its operations
//! nested in namespaces, its interfaces and its request kinds. This crate
//! is everything that is a pure function of definitions:
//!
//! - [`parse`] reads one from the `.d.ts` notation and
//!   [`parse_manifest`] from the manifest notation, and each checks it:
//!   one definition, whichever it was read from;
//! - [`print`] writes it as a `.d.ts` file again and [`print_manifest`]
//!   as a manifest ([`print_within`] and [`print_manifest_within`], when
//!   the text must be one a reader takes), [`canonical_manifest`] gives
//!   the bytes that name it, and [`declaration`] writes what a run's
//!   model reads of the surfaces it has;
//! - an [`Operation`]'s `check_args`, `check_answer` and `check_result`,
//!   and a [`Kind`]'s `check_request`, `check_message` and
//!   `check_result`, check JSON values against its types, each knowing
//!   which side of Clax's filling in of `Content` it runs on;
//!   [`check_held`] and [`check_sent`] check a value against a type that
//!   belongs to neither. Each answers with a [`ValueError`], which says
//!   whether the value is wrong, too large or too deep;
//! - [`Pattern`] says which operations an allow list names, and
//!   [`Kind::allows`] which a kind lets its runs have;
//!   [`Definition::run_operations`] says which a run of a kind is given,
//!   beside an operator's and a starter's lists, and which are withheld
//!   and why; [`Kind::instruction_places`] and [`Kind::content_places`]
//!   say where in a request the person's own words, and content, are;
//! - [`Definition::operations`] lists the operations by the dotted names
//!   a script, a journal and an allow list know them by;
//! - [`Reserved`] holds the names a host's surface may not take;
//! - [`wire`] holds the messages of the protocol a host registers and
//!   serves a surface with, as data.
//!
//! It depends on `serde` and `serde_json` alone and builds for wasm32.
//!
//! # Input from outside
//!
//! A host's file, a provider's answer and a script's arguments are all
//! written by someone else. Every reader takes [`Limits`], and a
//! definition keeps the ones it was read under: a file larger than the
//! limit is refused before it is read, every name and every list has a
//! longest length, nothing in a definition nests deeper than the limit,
//! and every refusal is a message of bounded length that shows no
//! character its reader would not see. (One refusal is as long as its
//! reader made it: of a call's arguments, an unknown field's name is
//! quoted whole, [`Operation::check_args`].)
//!
//! - **Time.** Reading a definition takes time proportional to the
//!   file's size, but for two steps that the limits bound instead:
//!   working out the fields each interface inherits, at most the number
//!   of interfaces times `list` times `list` steps, and putting its
//!   types in the form a check walks, at most [`Limits::plan_steps`]
//!   steps: a definition that needs more is refused. Checking a
//!   value does at most [`Limits::value_nodes`] units of work. One unit
//!   buys one comparison of a value with a type (not counting the
//!   values inside it), one look for a key among a type's fields, or
//!   one look at what an object holds under a name that tells a
//!   union's alternatives apart: each is at most a search among the
//!   keys of one object, among `list` names or literals of the
//!   definition, and among what the check has remembered. So a check
//!   within its budget takes time proportional to the units it spends,
//!   and a value that fits a type costs at most a few units for each of
//!   its values and keys, for each alternative of a union that it has
//!   to be tried against. A union whose object alternatives each say
//!   which they are in a field of literal type (`kind: "pull"`) costs
//!   one unit more than the alternative itself, however many
//!   alternatives it has. A union of three or more object alternatives
//!   with nothing to tell them apart is tried one alternative after
//!   another, and a value of the last costs more units than its JSON
//!   has bytes (in an array of `{"d":""}` against
//!   `{ a: string } | { b: string } | { c: string } | { d: string }`,
//!   ten units for the nine bytes of each), so a valid value of a
//!   megabyte can be refused as too large under the default budget. A
//!   manifest's reader also looks each key up among those of its
//!   object, and each name of a `required` among its fields: the size
//!   of the text times its logarithm, at most. A unit is not a
//!   fixed time: a comparison with a string literal, and a look for a
//!   key, cost the length of the string, which is time for the bytes
//!   of the value and not for the budget.
//! - **Memory.** A definition holds what its file says, and its types
//!   a second time in the form a check of a value walks. That form is
//!   built in the steps [`Limits::plan_steps`] counts, and a step
//!   keeps at most 128 bytes, beside the text of the file's string
//!   literals, which is kept twice: 13 MiB at the very most under the
//!   default limits, and 3 MiB for the largest that was found (256
//!   interfaces that inherit 128 fields with the longest names). While
//!   a manifest is read it is held a second time as a tree, with the
//!   way to each of its values, and its types a third time as they are
//!   built. [`Limits::manifest_values`] bounds how many values that
//!   is, and [`Limits::manifest_errors`] what is kept of its mistakes.
//!   Measured with an allocator that counts, in an optimised build, on
//!   texts as long as the default limits allow: a text that is refused
//!   takes 8.3 MiB of heap at its peak at the most that was found (one
//!   `enum` of 33,000 literals), and one that reads 13.6 MiB, of which
//!   7.1 MiB is the definition kept (an interface of 100 fields with
//!   names of 64 bytes, and as many interfaces as fit that extend it
//!   with 26 fields each, every one an `enum` of 20 literals). These
//!   are the most of the texts tried, not a bound: what a manifest
//!   that reads costs is its tree, its definition and the plan of its
//!   types together, and the limits above bound each. What
//!   tells the alternatives of a union apart is worked out and kept
//!   once for all the unions that list the same object types and
//!   interfaces in the same order, so a file of the default size whose
//!   78 unions each list the same 128 interfaces, of 32 telling fields
//!   each, keeps under 1 MiB; one that lists them in 78 orders needs
//!   more steps than the default allows, and is refused. A check of a
//!   value keeps its place in the value ([`Limits::value_depth`]
//!   levels at most), and remembers what it found out about the arrays
//!   and objects inside a value that it tries against several
//!   alternatives of a union: one entry for each unit spent there at
//!   most, so [`Limits::value_nodes`] bounds a check's memory too,
//!   at about 21 MiB under the default in the worst case that was
//!   found.
//! - **Stack.** What reads, checks and prints a definition recurses as
//!   deep as its types and namespaces nest, which is at most
//!   [`Limits::depth`], itself at most [`Limits::MAX_DEPTH`]; in a
//!   build without optimisation a level costs up to 8 KiB of stack.
//!   Under the default limits everything in this crate runs in 512 KiB
//!   of stack, and with `depth` at its hard maximum, in 1 MiB: about
//!   twice what the `.d.ts` reader, which needs the most, was measured
//!   to use (259 KiB and 499 KiB). A manifest's reader, on
//!   the deepest manifest each allows, was run on threads of a given
//!   stack in steps of 16 KiB, in Bazel's build without optimisation
//!   (`fastbuild`, aarch64 macOS): it runs in 320 KiB and not in
//!   304 KiB, and in 624 KiB and not in 608 KiB. `serde_json` reads the
//!   text some 1.7 KiB of stack for each array and object around a
//!   value, and a manifest nests them [`Limits::manifest_nesting`]
//!   deep at the most (136 and 264). Its writer ran in 64 KiB and in
//!   128 KiB. A check of a value does not recurse into
//!   the value: it keeps its place on the heap, so a value as deep as
//!   [`Limits::value_depth`] allows costs no more stack than a flat
//!   one, and a check against a type that nests a few levels runs in
//!   64 KiB. (Dropping a `serde_json::Value` or a [`Ty`] that was built
//!   deeper than these limits, by other means than this crate's
//!   readers, is its owner's to do with care.)
//!
//! # The `.d.ts` notation
//!
//! A file holds one `declare const <name>: { … };` and `interface`s, in
//! any order. `//` and `/* */` comments are not part of a definition; a
//! doc comment (`/** */`) is. A line ends at a line feed, a carriage
//! return, both together, U+2028 or U+2029; a byte-order mark at the
//! start is skipped; no other control character but a tab may appear,
//! and no character that shows nothing or changes the direction of
//! text (listed under "The checks"), in a `//` or `/* */` comment
//! either: such a character there can make an editor show a reviewer
//! other declarations than the ones that are read.
//!
//! ```ts
//! /** Rules for every run that has the surface. */
//! declare const shell: {
//!   /** The windows open on the desktop. @read */
//!   windows(): Promise<Window[]>;
//!   /** Rules for runs that have one of this namespace's operations. */
//!   settings: {
//!     /** Changes appearance settings. @write @throws invalid */
//!     set(patch: { theme?: "light" | "dark" }): Promise<null>;
//!   };
//!   /** Asks the person. @effect @waits @once @throws declined @throws busy */
//!   ask(question: string, choices?: string[]): Promise<Instruction>;
//! };
//! interface Window extends Named {
//!   /** The workspace it is on. */
//!   workspace: Int;
//! }
//! interface Named { id: string; title: string }
//! interface Kinds {
//!   /**
//!    * The prompt's paragraph about the request.
//!    * @allow shell.windows shell.settings.*
//!    */
//!   triage(request: { ask: Instruction }, message?: string): { summary: string };
//! }
//! ```
//!
//! - **Operations** are methods that return `Promise<T>`, with positional
//!   parameters, the optional ones (`name?: T`) last. Each has a doc
//!   comment: prose that says what it does, then its tags.
//! - **Doc comments.** In a comment of several lines, a line's leading
//!   `* ` is not part of it, and what follows keeps its indentation; a
//!   line written without one is taken as it stands, without the white
//!   space around it. A word that starts with `@` begins a tag, and
//!   everything after the first tag is tags, so tags follow the prose
//!   (prose that needs such a word writes it in backticks).
//! - **Tags.** An operation takes exactly one of `@read`, `@write`,
//!   `@effect` and `@spend`, and optionally `@waits`, `@once` and
//!   `@throws`, which takes one error code (`not_found`: lowercase
//!   letters, digits and `_`) and is written again for each further
//!   code: `@throws invalid_path @throws not_found`. A kind takes
//!   `@allow` with its [`Pattern`]s, and may write it again for more;
//!   `@allow` alone allows no operation. Nothing else takes a tag, and
//!   no other tag exists.
//! - **Types** are `string`, `number`, `Int`, `boolean`, `null`,
//!   `unknown`, string literals in double quotes, `true`, `false`, `T[]`,
//!   `Record<string, T>`, object types written inline, unions, the file's
//!   interfaces by name, and `Instruction`, `Content` and `Image`. Type
//!   aliases, other generics, `any` and the rest of TypeScript are
//!   refused.
//! - **Interfaces** may `extends` others and document themselves and
//!   their fields; an object type written inline documents nothing.
//! - **Kinds** are the methods of `interface Kinds`:
//!   `name(request: T): R`, or `name(request: T, message?: M): R` for a
//!   kind that takes messages after it starts. The parameters have those
//!   names.
//!
//! # The manifest notation
//!
//! The same definition as one JSON document, for a program to build and
//! send: a host that registers at run time, a page's script, a bridge
//! from an MCP server. It says what a `.d.ts` file says and nothing
//! more: [`parse_manifest`] reads it to the definition [`parse`] reads
//! the file to, under the same checks, and each notation's writer
//! writes what the other's reader read, for every definition there is.
//! The file above is this manifest, as [`print_manifest`] lays it out:
//!
//! ```json
//! {
//!   "clax_surface": 1,
//!   "name": "shell",
//!   "rules": "Rules for every run that has the surface.",
//!   "operations": [
//!     {
//!       "name": "windows",
//!       "description": "The windows open on the desktop.",
//!       "access": "read",
//!       "params": [],
//!       "result": {"type": "array", "items": {"$ref": "#/interfaces/Window"}}
//!     },
//!     {
//!       "namespace": "settings",
//!       "rules": "Rules for runs that have one of this namespace's operations."
//!     },
//!     {
//!       "name": "settings.set",
//!       "description": "Changes appearance settings.",
//!       "access": "write",
//!       "throws": ["invalid"],
//!       "params": [
//!         {"name": "patch", "schema": {"type": "object", "properties": {"theme": {"enum": ["light", "dark"]}}, "additionalProperties": false}}
//!       ],
//!       "result": {"type": "null"}
//!     },
//!     {
//!       "name": "ask",
//!       "description": "Asks the person.",
//!       "access": "effect",
//!       "waits": true,
//!       "once": true,
//!       "throws": ["declined", "busy"],
//!       "params": [
//!         {"name": "question", "schema": {"type": "string"}},
//!         {"name": "choices", "optional": true, "schema": {"type": "array", "items": {"type": "string"}}}
//!       ],
//!       "result": {"$ref": "clax:Instruction"}
//!     }
//!   ],
//!   "interfaces": {
//!     "Window": {
//!       "type": "object",
//!       "extends": ["Named"],
//!       "properties": {
//!         "workspace": {"type": "integer", "description": "The workspace it is on."}
//!       },
//!       "required": ["workspace"],
//!       "additionalProperties": false
//!     },
//!     "Named": {
//!       "type": "object",
//!       "properties": {
//!         "id": {"type": "string"},
//!         "title": {"type": "string"}
//!       },
//!       "required": ["id", "title"],
//!       "additionalProperties": false
//!     }
//!   },
//!   "kinds": [
//!     {
//!       "name": "triage",
//!       "description": "The prompt's paragraph about the request.",
//!       "allow": ["shell.windows", "shell.settings.*"],
//!       "request": {"type": "object", "properties": {"ask": {"$ref": "clax:Instruction"}}, "required": ["ask"], "additionalProperties": false},
//!       "message": {"type": "string"},
//!       "result": {"type": "object", "properties": {"summary": {"type": "string"}}, "required": ["summary"], "additionalProperties": false}
//!     }
//!   ]
//! }
//! ```
//!
//! ## Every member
//!
//! In every object of a manifest, a member this table does not name is
//! refused, and each member is given once.
//!
//! | Of | Member | | Is | Left out, it means |
//! |---|---|---|---|---|
//! | The document | `clax_surface` | required | The notation's version: `1` ([`MANIFEST_VERSION`]) | |
//! | | `name` | required | The surface's name | |
//! | | `rules` | optional | The surface's rules | No rules |
//! | | `operations` | optional | Array: operations and namespaces' entries, in order | None (`[]`) |
//! | | `interfaces` | optional | Object: an interface under each name, in order | None (`{}`) |
//! | | `kinds` | optional | Array: the kinds, in order | None (`[]`) |
//! | An operation | `name` | required | Its dotted name below the surface: `settings.set` | |
//! | | `description` | required | Its documentation | |
//! | | `access` | required | `"read"`, `"write"`, `"effect"` or `"spend"` | |
//! | | `waits`, `once` | optional | `true` or `false` | `false` |
//! | | `throws` | optional | Array of error codes | None (`[]`) |
//! | | `params` | optional | Array of parameters, in order | None (`[]`) |
//! | | `result` | required | A type | |
//! | A parameter | `name` | required | Its name | |
//! | | `schema` | required | A type | |
//! | | `optional` | optional | `true` or `false` | `false` |
//! | A namespace's entry | `namespace` | required | Its dotted name below the surface | |
//! | | `rules` | optional | Its rules | No rules |
//! | An interface | `type` | required | `"object"` | |
//! | | `additionalProperties` | required | `false` | |
//! | | `properties` | optional | Object: a type under each field's name, in order | No fields (`{}`) |
//! | | `required` | optional | Array of the names of the fields that are not optional | Every field optional (`[]`) |
//! | | `extends` | optional | Array of interfaces' names | None (`[]`) |
//! | | `description` | optional | Its doc comment | None |
//! | A kind | `name`, `description` | required | Its name; the prompt's paragraph about the request | |
//! | | `request`, `result` | required | Types | |
//! | | `message` | optional | A type | The kind takes no messages |
//! | | `allow` | required of a host's kind | Array of [`Pattern`]s; `[]` allows no operation | Clax's own surface only: the kind narrows nothing |
//!
//! - **The version.** `clax_surface` says which version of the notation
//!   the manifest is written in; one that gives another, or none, is
//!   refused before anything else of it is read. A definition has no
//!   version of its own: its canonical bytes name it.
//! - **Operations** are listed flat, by dotted name below the surface:
//!   `settings.set`, not `shell.settings.set` (a name that starts with
//!   the surface's own is refused). An allow pattern names the surface
//!   too, since a kind may allow another surface's operations:
//!   `shell.settings.*`.
//! - **Namespaces** follow from the dotted names. A namespace stands
//!   among the members of the surface or namespace that holds it where
//!   the first of its own members is listed, and its members follow one
//!   another. One that has rules, or no member, has an entry of its own
//!   in `operations`, before its members:
//!   `{"namespace": "settings", "rules": "…"}`.
//! - **Types** are a subset of JSON Schema. `{}` is `unknown`;
//!   `{"type": "string"}`, and so `"number"`, `"boolean"` and `"null"`;
//!   `{"type": "integer"}` is `Int`; `{"const": "a"}` and
//!   `{"const": true}` are literals, and `{"enum": ["a", "b"]}` a union
//!   of literals; `{"type": "array", "items": T}` is `T[]`;
//!   `{"type": "object", "additionalProperties": T}` is
//!   `Record<string, T>`;
//!   `{"type": "object", "properties": {…}, "required": […], "additionalProperties": false}`
//!   is an object type, whose fields are optional but those `required`
//!   names; `{"anyOf": [A, B]}` is a union;
//!   `{"$ref": "#/interfaces/Name"}` is an interface of the manifest,
//!   whole (a reference into one is refused);
//!   `{"$ref": "clax:Instruction"}`, `clax:Content` and `clax:Image` are
//!   the core's types. Everything else of JSON Schema is refused, by
//!   its keyword and where it is: `minimum`, `pattern`, `format`,
//!   `oneOf`, `allOf`, `default`, `title`, a `type` that is an array,
//!   `"additionalProperties": true`, a `$ref` to anywhere else. A range
//!   a type cannot say is the provider's to check.
//! - **An object type says `"additionalProperties": false`.** A schema
//!   that says nothing of it takes, in JSON Schema, any further field;
//!   the format's object types take none. Reading such a schema as
//!   closed would give it another meaning than its author's tools give
//!   it, without a word; so it is refused, at the object, with what to
//!   add.
//! - **Interfaces** are the members of `interfaces`. `extends` is the
//!   notation's own keyword: JSON Schema's `allOf` of two closed object
//!   types takes no value at all, so it would not say what an interface
//!   means; `properties` lists the interface's own fields. Nothing but
//!   an interface has a name: any other type is written where it is
//!   used.
//! - **Documentation.** An operation's and a kind's `description` is
//!   required and is what the model reads; an interface's, and that of
//!   a field of an interface (in the field's type, beside its other
//!   keywords), is optional; `rules` are a surface's and a namespace's.
//!   No other type takes a `description`: nothing would read it. Each
//!   is text of one line or several (line feeds), with tabs and no
//!   other control character, no white space at either end or at a
//!   line's end, not empty, without `*/`, without a character that
//!   shows nothing, and without a word that starts with `@` (write it
//!   in backticks): the same rules as a `.d.ts` file's doc comments,
//!   since every definition is written as one for its model.
//! - **Spellings the reader takes and the writer does not write.**
//!   `operations`, `params` and `properties` left out, for none; an
//!   empty `throws`, `required`, `extends`, `interfaces` or `kinds`,
//!   and `waits`, `once` or `optional` as `false`, where leaving the
//!   member out says the same; an `enum` of one literal, for a `const`;
//!   `"type": "string"` or `"type": "boolean"` beside an `enum` or a
//!   `const` whose literals are all that; an `anyOf` inside an `anyOf`,
//!   which is the one union of all their alternatives;
//!   `{"$ref": "clax:Int"}`, for `{"type": "integer"}`; the entry of a
//!   namespace that needs none; the members of any object in another
//!   order than the table's, and the names of `required` in any order.
//!   Each reads to the definition its other spelling reads to. A
//!   spelling is bounded as it is written: a nested `anyOf`, or an
//!   `enum` inside an `anyOf`, nests two arrays and objects deeper than
//!   the writer's spelling of the same union, so at the deepest level a
//!   type may have it can be past [`Limits::manifest_nesting`] where
//!   the writer's is not.
//! - **Property order is document order.** The order of `operations`,
//!   of the interfaces, of an object type's `properties`, of a union's
//!   alternatives and of `kinds` is the definition's: the model reads
//!   them in it, and it is part of what names the definition. So a
//!   program writes them in an order that is the same each time it
//!   runs. One that builds its manifest in a map without an order of
//!   its own (a hash map) sends another definition on each start; one
//!   whose map sorts its keys (`serde_json::Value`, as this workspace
//!   builds it) sends its fields in the alphabet's order, which is then
//!   their order, and is not the order of its `.d.ts` file.
//! - **The text** is JSON in UTF-8, [`Limits::manifest_bytes`] long at
//!   the most, of [`Limits::manifest_values`] values at the most, with
//!   each key of an object once, and no more than
//!   [`Limits::manifest_nesting`] arrays and objects around any value:
//!   as deep as the deepest type the limits allow is written. One
//!   byte-order mark before it is skipped.
//! - **Refusals.** [`parse_manifest`] answers with every entry's first
//!   mistake ([`Errors`]), in the order the text has them: a manifest
//!   is written by a program, which gets wrong every operation it makes
//!   the same way. It says [`Limits::manifest_errors`] of them at the
//!   most, and then how many entries it did not look at, so that what
//!   it answers is small whatever the text. Text that is not JSON, and
//!   a document of another version, is one mistake. Once
//!   every entry reads, the rules of a definition are checked ("The
//!   checks"), and the first that is broken is said, in a manifest's
//!   words. Each has its [`Place`]: the line, for text that is not
//!   JSON; else the value, as a JSON Pointer
//!   (`/operations/2/params/0/schema/minimum`).
//! - **Size.** A manifest is two to three times its `.d.ts` file for
//!   surfaces written as people write them (2.2 to 2.9 for the three in
//!   this repository, and for one of them repeated up to the limit on a
//!   file), which is why [`Limits::manifest_bytes`] is four times
//!   [`Limits::file_bytes`]. A file written to be dense (no
//!   documentation, one-letter names, fields of empty object types
//!   nested as deep as types go) has a manifest of up to 22 times its
//!   size, and no manifest under the default limits when it is longer
//!   than some 11 KiB of that: [`print_manifest_within`] says so.
//!
//! ## What names a definition
//!
//! [`canonical_manifest`] gives a definition's canonical bytes: its
//! manifest with one spelling for each meaning, its members in one
//! order, and nothing between its tokens. Two definitions are equal
//! exactly when their canonical bytes are, so a hash of them names a
//! definition (hashing is the caller's: this crate has no hash). The
//! form is fixed for version 1 of the notation and is spelled out on
//! that function. For the example above it starts:
//!
//! ```text
//! {"clax_surface":1,"name":"shell","rules":"Rules for every run that has the surface.","operations":[{"name":"windows","description":"The windows open on the desktop.","access":"read","params":[],"result":{"type":"array","items":{"$ref":"#/interfaces/Window"}}},{"namespace":"settings",
//! ```
//!
//! Everything a definition says is in them: its documentation, and the
//! order of its operations, interfaces, fields and kinds, as much as
//! its types. Nothing else is: a manifest may be laid out any way and
//! spelled any way the reader takes, and the bytes that name it are
//! those of the definition read from it, not its own.
//! [`print_manifest`] lays the same document out for a person; that
//! layout is no contract, and nothing should be keyed on it.
//!
//! ## From an MCP server
//!
//! An MCP server's tools can be a surface's operations, through a
//! bridge that writes the manifest; the tool list itself is not one.
//! Servers write JSON Schema freely, and a definition holds a subset:
//! of five tools written as zod, pydantic and hand-written servers
//! write them, none reads as it is listed (the crate's tests keep the
//! five, and a bridge of this kind over them). What a bridge does:
//!
//! - **Names.** A tool's name becomes an identifier (`list-windows` and
//!   `http.request` are not: `list_windows`, `http_request`), and so
//!   does every field's; an object with a field that cannot be named
//!   (`content-type`) is `Record<string, unknown>`.
//! - **Words.** A tool with no description is given one. A description
//!   is made documentation: on the rules above, a word that starts with
//!   `@` goes in backticks, and one longer than [`Limits::doc_bytes`]
//!   is cut.
//! - **Arguments.** A tool takes one object of named arguments, and an
//!   operation positional parameters: the operation has one parameter,
//!   `args`, whose type is an interface made of the tool's
//!   `inputSchema`, so that its fields keep their descriptions.
//! - **Types.** Every object type is closed
//!   (`"additionalProperties": false`), at every level. Keywords the
//!   subset lacks are dropped (`minLength`, `minimum`, `format`,
//!   `default`, `title`, `$schema`); `oneOf` and a `type` that is a
//!   list become `anyOf`; a `$ref` into `$defs` is replaced by what it
//!   names, or `$defs` become interfaces; an `enum` of numbers, and
//!   anything else the subset cannot say, becomes `{}` (`unknown`),
//!   which takes any value.
//! - **Results.** `result` is `{}`, unless the server's results are
//!   known to be closed: a result is checked against its type
//!   ([`Operation::check_result`]), and an object with a field its type
//!   does not list is refused, where an `outputSchema` as servers write
//!   it allows one.
//! - **Access.** `"read"` when the server says the tool only reads
//!   (`readOnlyHint`), else `"write"`, or `"effect"` for what a person
//!   sees.
//!
//! What is lost: every range, length, pattern, format and default,
//! which the server still checks and refuses by; the descriptions of
//! fields below an interface's own; and the shape of whatever became
//! `unknown`. A tool list is also large: one of more than some hundred
//! tools is past [`Limits::operations`] or [`Limits::manifest_bytes`],
//! and is several surfaces.
//!
//! # The checks
//!
//! [`parse`] and [`parse_manifest`] each refuse a text that breaks
//! their notation. The rules below are not a notation's: they hold for
//! every [`Definition`], whatever it was read from, and are checked on
//! the definition itself.
//!
//! - It is within its [`Limits`]. No name is longer than
//!   [`Limits::name_bytes`].
//! - The surface's name is one lowercase word, and not `core` or
//!   `rules`, which begin the names of the core's slots in a prompt
//!   file; a host's ([`Author::Host`]) is not [`Reserved`] and not a
//!   name a run's step keeps a field under ([`STEP_FIELDS`]), and none
//!   of its operations is `@spend`.
//! - A surface or namespace holds each name once. A namespace's and an
//!   operation's name is an identifier a script can call: not
//!   `__proto__`, `constructor` or `prototype`, and not `then`. An
//!   interface's, a field's and a kind's is an identifier other than
//!   those three; a parameter's is an identifier.
//! - What the model reads is TypeScript. A surface, an interface and a
//!   parameter are not named with one of TypeScript's reserved words
//!   (`class`, `new`, `this`, `default`, `in`, `typeof`, `keyof` and
//!   the rest: ECMAScript's reserved words, `undefined` and the type
//!   operators), nor a surface with one ECMAScript reserves in strict
//!   mode (`let`, `static`, `interface`, `package` and the rest); an
//!   interface is not named as a type of the format or of TypeScript
//!   either. The interfaces of the surface named `clax` are declared
//!   at the top level of what the model reads, so none of them is
//!   named as a global type of TypeScript's standard library without
//!   the DOM (`String`, `Date`, `Error`, `Map`, `Partial`, `Awaited`
//!   and the rest of `lib.es2022`), which it would merge or clash
//!   with; any other surface's are in a namespace of their own. A namespace, an operation, a kind and a
//!   field are properties, which any word may name (`db.delete`), but
//!   an operation or a kind named `new` would be a constructor.
//! - An operation and a kind have documentation. No doc comment is
//!   empty, holds `*/`, a control character other than a tab or a line
//!   feed, or a word that starts with `@`, or has white space at either
//!   end or at the end of a line: it prints as one comment and reads
//!   back the same.
//! - What a model reads is what a reviewer sees. No text of a
//!   definition holds a character that shows nothing or changes the
//!   direction of the text around it, whoever its author and whichever
//!   notation it was read from: not its rules, its documentation and
//!   comments, nor a string literal (a `const`, an `enum`'s values).
//!   Names, error codes and allow patterns are ASCII, so cannot hold
//!   one; a `.d.ts` file holds none anywhere, in a `//` or `/* */`
//!   comment either. One function decides (`check::hidden`), and this
//!   table is all it refuses, every code point Unicode says shows
//!   nothing by default (its general category Cf and its
//!   Default_Ignorable_Code_Point property, as of Unicode 17.0, with
//!   the unassigned code points inside the ranges):
//!
//!   | Code points | What they are |
//!   |---|---|
//!   | U+00AD | Soft hyphen |
//!   | U+034F | Combining grapheme joiner |
//!   | U+0600 to U+0605, U+06DD, U+070F, U+0890, U+0891, U+08E2 | Arabic and Syriac signs that span numbers |
//!   | U+061C | Arabic letter mark (a direction mark) |
//!   | U+115F, U+1160, U+3164, U+FFA0 | Hangul fillers |
//!   | U+17B4, U+17B5 | Khmer inherent vowels |
//!   | U+180B to U+180F | Mongolian variation selectors and vowel separator |
//!   | U+200B | Zero width space |
//!   | U+200C, U+200D | Zero width non-joiner and joiner |
//!   | U+200E, U+200F | Left-to-right and right-to-left marks |
//!   | U+202A to U+202E | Direction embeddings, overrides and their end |
//!   | U+2060 to U+206F | Word joiner, invisible operators, direction isolates (U+2066 to U+2069), deprecated format characters |
//!   | U+FE00 to U+FE0F | Variation selectors |
//!   | U+FEFF | Zero width no-break space (skipped as a byte-order mark at the start of a text) |
//!   | U+FFF0 to U+FFFB | Interlinear annotation characters, and code points kept for more |
//!   | U+110BD, U+110CD | Kaithi number signs |
//!   | U+13430 to U+1343F | Egyptian hieroglyph format controls |
//!   | U+1BCA0 to U+1BCA3 | Shorthand format controls |
//!   | U+1D173 to U+1D17A | Musical symbol format controls |
//!   | U+E0000 to U+E0FFF | Tag characters, the variation selectors supplement (U+E0100 to U+E01EF), and code points kept for more |
//!
//!   Beside these, no control character but a tab and a line feed (in
//!   documentation) and no U+2028 or U+2029 appears. A refusal names
//!   such a character by its code and shows none. The joiners, U+200C
//!   and U+200D, are refused with the rest although some text needs
//!   them: documentation cannot hold an emoji joined from several by
//!   U+200D or one that needs U+FE0F to draw in colour, nor a word of a
//!   script that writes U+200C or U+200D between its letters (Persian,
//!   and several Indic scripts). Telling where a script needs one would
//!   take Unicode's joining data, and would leave a character a
//!   reviewer cannot see open for anything else; such text is written
//!   without them, and a model reads it as well.
//! - An error code is lowercase letters, digits and `_`, starting with a
//!   letter, and an operation names each once.
//! - No required parameter follows an optional one, no two parameters of
//!   an operation share a name, and no required parameter's type admits
//!   `null`, which a call passes to leave a parameter out.
//! - `@once` is not on a `@read`: repeating a read is harmless, so the
//!   two contradict each other.
//! - Each interface is declared once, under a name that is not one of
//!   the format's own; what it `extends` is declared, and `extends` does
//!   not come back to where it started; it has each field once, counting
//!   those it inherits.
//! - Every type name is an interface of the file; an object type lists
//!   each field once and documents none; a string literal holds no
//!   control character and no line end; a union has two alternatives or
//!   more, each once.
//! - Each kind is declared once. A host's kind says which operations
//!   its runs may have (`@allow`, `"allow"`), and an empty list allows
//!   none; only a kind of Clax's own surface ([`Author::Clax`]) may say
//!   nothing, which narrows nothing ([`Kind::allows`]).
//! - A kind's result is an object: an interface or an object type
//!   written where it is used. A union (a nullable object too), an
//!   array, a primitive, a literal, `null`, `unknown`, `Instruction`,
//!   `Content`, `Image` and a `Record<string, T>` are refused, for Clax's
//!   own surface and a host's alike: the value a run finishes with is
//!   the input of the tool that ends it ([`Kind::result_schema`]), which
//!   the model APIs take only as an object, and a `Record`'s schema, an
//!   object with `additionalProperties` and no `properties`, is not one
//!   every API takes. Wrap the value in a field.
//! - A host's kind's request has no required field whose type admits
//!   `null`: a field that may be absent is optional (`name?:`), which
//!   may be left out or be `null`. The fields are the request's own, of
//!   the object type or interface it is (or of each one among a
//!   union's alternatives); Clax's own surface is not held to this.
//! - `Instruction` appears only in a kind's request, in a kind's message
//!   and in the result of an operation tagged `@waits`: not in any
//!   parameter, not in another operation's result, not in a kind's
//!   result, whether written there or in an interface reached from
//!   there.
//!
//! What a definition cannot say, the format cannot hold: an operation has
//! one access, a kind one request and at most one message, a union is
//! never empty.

mod allow;
mod check;
mod dts;
mod error;
mod limits;
mod manifest;
mod model;
mod plan;
mod print;
mod run_ops;
#[cfg(test)]
mod tests;
mod value;
pub mod wire;

pub use allow::{Pattern, RESERVED_CALLABLE};
pub use check::{Author, Reserved, STEP_FIELDS, hidden};
pub use dts::parse;
pub use error::{Error, Errors, Place};
pub use limits::Limits;
pub use manifest::{
    MANIFEST_VERSION, canonical_manifest, parse_manifest, print_manifest, print_manifest_within,
};
pub use model::{
    Access, Alts, CLAX, Definition, Field, Interface, Kind, Member, Namespace, Op, Operation,
    Param, Ty,
};
pub use print::{declaration, print, print_within};
pub use run_ops::{ContentUse, RunOperations, RunScope, Withheld};
pub use value::{IMAGE_MEDIA_TYPES, ValueError, ValueErrorKind, check_held, check_sent};
