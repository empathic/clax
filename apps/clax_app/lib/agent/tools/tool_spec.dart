/// How a tool is declared to the model.
library;

/// Every tool's name, in the one order a run declares them. The order never
/// changes within a run, so the tools stay a cached prefix.
const toolOrder = [
  'list_files',
  'read_file',
  'write_file',
  'str_replace',
  'delete_file',
  'set_meta',
  'db_collections',
  'db_list',
  'db_get',
  'db_query',
  'db_write',
  'render',
  'ask',
  'finish',
  'give_up',
];

/// A tool's name, description and JSON Schema input.
///
/// A [strict] tool's input is validated by the API against [inputSchema];
/// its schema sets `additionalProperties: false` and requires every
/// property except those made with [optional], which may be left out. A
/// strict schema has no `anyOf` and no `null` type: the API caps union
/// types and optional properties across a request's strict tools
/// ([strictLimits]). An [eager]
/// tool's input streams before it is whole (`eager_input_streaming`); the
/// API cannot validate such input, so an eager tool is never strict and
/// checks its own input.
class ToolSpec {
  const ToolSpec(
    this.name,
    this.description,
    this.inputSchema, {
    this.strict = true,
    this.eager = false,
  });

  final String name;
  final String description;
  final Map<String, Object?> inputSchema;
  final bool strict;
  final bool eager;

  /// The Messages API form: `{name, description, input_schema}` plus
  /// `strict: true` or `eager_input_streaming: true`.
  Map<String, Object?> toMessagesApi() => {
    'name': name,
    'description': description,
    'input_schema': inputSchema,
    if (strict && !eager) 'strict': true,
    if (eager) 'eager_input_streaming': true,
  };

  /// The MCP form: `{name, description, inputSchema}`.
  Map<String, Object?> toMcp() => {
    'name': name,
    'description': description,
    'inputSchema': inputSchema,
  };
}

/// An object schema with [properties], all required except those wrapped
/// in [optional], and no others.
Map<String, Object?> objectSchema(Map<String, Object?> properties) => {
  'type': 'object',
  'properties': {
    for (final e in properties.entries)
      e.key: switch (e.value) {
        final Optional o => o.schema,
        final v => v,
      },
  },
  'required': [
    for (final e in properties.entries)
      if (e.value is! Optional) e.key,
  ],
  'additionalProperties': false,
};

/// A property [objectSchema] leaves out of `required`; the tool reads a
/// missing key as null.
final class Optional {
  const Optional(this.schema);

  final Map<String, Object?> schema;
}

/// An optional property of JSON type [type].
Optional optional(String type, {String? description}) =>
    Optional({'type': type, 'description': ?description});

/// The Messages API's caps across the strict tools of one request: the
/// count of strict tools, of optional properties and of properties with
/// union types (`anyOf` or a list of types).
const strictLimits = (tools: 20, optional: 24, unions: 16);
