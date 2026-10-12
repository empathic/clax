/// The JSON schema of an [EditResult](edit_result.dart), shared by every
/// backend: the Messages API takes it as structured output
/// (`output_config.format`), `claude -p` as `--json-schema`.
///
/// It is strict-structured-output compatible: every object has
/// `additionalProperties: false` and lists all of its properties as
/// required; optional values are `anyOf [..., null]`. Document data and the
/// capability declaration travel as JSON strings (`data_json`,
/// `capabilities_json`), since free-form objects are not allowed.
library;

const editSchema = <String, Object?>{
  "type": "object",
  "additionalProperties": false,
  "required": [
    "reply",
    "title",
    "files",
    "edits",
    "removed",
    "db",
    "capabilities_json",
    "resolve",
  ],
  "properties": {
    "reply": {
      "type": "string",
      "description":
          "One or two sentences to the person: what you made or changed.",
    },
    "title": {
      "anyOf": [
        {"type": "string"},
        {"type": "null"},
      ],
      "description":
          "The app's name (2–4 words). Required for a new app; null to keep.",
    },
    "files": {
      "type": "array",
      "description": "Whole files to create or replace. A new app must include index.html.",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["path", "content"],
        "properties": {
          "path": {"type": "string"},
          "content": {"type": "string"},
        },
      },
    },
    "edits": {
      "type": "array",
      "description": "Exact replacements in existing files; old_str must occur exactly once.",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["path", "old_str", "new_str"],
        "properties": {
          "path": {"type": "string"},
          "old_str": {"type": "string"},
          "new_str": {"type": "string"},
        },
      },
    },
    "removed": {
      "type": "array",
      "items": {"type": "string"},
    },
    "db": {
      "type": "array",
      "description": "Document writes to seed or migrate data.",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["op", "collection", "doc_id", "data_json"],
        "properties": {
          "op": {
            "enum": ["set", "update", "delete"],
          },
          "collection": {"type": "string"},
          "doc_id": {"type": "string"},
          "data_json": {
            "anyOf": [
              {"type": "string"},
              {"type": "null"},
            ],
            "description": "A JSON object as a string; null for delete.",
          },
        },
      },
    },
    "capabilities_json": {
      "anyOf": [
        {"type": "string"},
        {"type": "null"},
      ],
      "description":
          "The full capability declaration as a JSON object string, e.g. "
          "{\"db\":{}}; null keeps the current one.",
    },
    "resolve": {
      "type": "boolean",
      "description": "True when the comments are fully addressed.",
    },
  },
};
