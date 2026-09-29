// A stand-in for the subset of Pi's ExtensionAPI the extension uses (`on`,
// `registerTool`, `registerCommand`), capturing what it registers so tests can
// fire events, call tools, and run commands the way Pi does.
import type { ExtensionAPI, ExtensionCommandContext, ExtensionContext, ToolDefinition } from "@mariozechner/pi-coding-agent";

type Handler = (event: unknown, ctx: ExtensionContext) => unknown;
type Command = { description?: string; handler: (args: string, ctx: ExtensionCommandContext) => Promise<void> };

/** A tool result as Pi's agent loop reports it to the model. */
export interface ToolOutcome {
  content: { type: string; text?: string }[];
  isError: boolean;
}

export interface FakeContext {
  ctx: ExtensionCommandContext;
  /** Messages passed to `ctx.ui.notify`. */
  notes: { message: string; type?: string }[];
}

/** A context for a session with ID `sessionId` working in `cwd`. */
export function fakeContext(cwd: string, sessionId: string): FakeContext {
  const notes: { message: string; type?: string }[] = [];
  const ctx = {
    cwd,
    hasUI: false,
    sessionManager: { getSessionId: () => sessionId },
    ui: { notify: (message: string, type?: string) => { notes.push({ message, type }); } },
  } as unknown as ExtensionCommandContext;
  return { ctx, notes };
}

export class FakePi {
  readonly handlers = new Map<string, Handler[]>();
  readonly tools = new Map<string, ToolDefinition>();
  readonly commands = new Map<string, Command>();

  get api(): ExtensionAPI {
    const api = {
      on: (event: string, handler: Handler) => {
        this.handlers.set(event, [...(this.handlers.get(event) ?? []), handler]);
      },
      registerTool: (tool: ToolDefinition) => { this.tools.set(tool.name, tool); },
      registerCommand: (name: string, options: Command) => { this.commands.set(name, options); },
    };
    return api as unknown as ExtensionAPI;
  }

  /** Runs every handler for `event`, in registration order, awaiting each. */
  async emit(event: string, payload: unknown, ctx: ExtensionContext): Promise<void> {
    for (const h of this.handlers.get(event) ?? []) await h(payload, ctx);
  }

  /** Calls tool `name` as Pi's agent loop does: a thrown error becomes a text
   * result holding its message, flagged `isError`. */
  async callTool(name: string, params: unknown, ctx: ExtensionContext): Promise<ToolOutcome> {
    const tool = this.tools.get(name);
    if (!tool) throw new Error(`no tool ${name}`);
    try {
      const r = await tool.execute("call-1", params as never, undefined, undefined, ctx);
      return { content: r.content as ToolOutcome["content"], isError: false };
    } catch (e) {
      return { content: [{ type: "text", text: e instanceof Error ? e.message : String(e) }], isError: true };
    }
  }

  async runCommand(name: string, args: string, ctx: ExtensionCommandContext): Promise<void> {
    const cmd = this.commands.get(name);
    if (!cmd) throw new Error(`no command ${name}`);
    await cmd.handler(args, ctx);
  }
}

/** The JSON object in a tool result's single text block. */
export function json(outcome: ToolOutcome): Record<string, any> {
  const text = outcome.content[0]?.text;
  if (outcome.content.length !== 1 || typeof text !== "string") throw new Error("expected one text block");
  return JSON.parse(text);
}
