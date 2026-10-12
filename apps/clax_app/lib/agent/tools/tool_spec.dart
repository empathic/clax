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
/// property (an optional one is `anyOf` its type and `null`). An [eager]
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

/// An object schema with [properties], all required and no others.
Map<String, Object?> objectSchema(Map<String, Object?> properties) => {
  'type': 'object',
  'properties': properties,
  'required': properties.keys.toList(),
  'additionalProperties': false,
};

/// A property of JSON type [type] that may also be null.
Map<String, Object?> nullable(String type, {String? description}) => {
  'anyOf': [
    {'type': type},
    {'type': 'null'},
  ],
  'description': ?description,
};
