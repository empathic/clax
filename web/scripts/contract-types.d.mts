/** web/sdk/openapi.json, the API contract the routes generate (contract-types.mjs). */
export const CONTRACT: string;
/** web/sdk/src/contract.gen.ts, the types it generates. */
export const GENERATED: string;
/** The TypeScript type of a JSON Schema of the contract, at `indent`. */
export function typeOf(schema: unknown, indent?: string): string;
/** contract.gen.ts of the parsed contract. */
export function generate(contract: unknown): string;
