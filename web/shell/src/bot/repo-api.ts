// A queue's repository link and the owner's view of them all, for the panel
// and Admin › Repositories, which alone load this module. Credentials are
// write-only: no response carries one, only its `KeyInfo`.
import { type KeyInfo, call } from "./api";

const e = encodeURIComponent;
const repoPath = (qid: string) => `/api/queues/${e(qid)}/repo`;
/** What a repository link waits for, if anything. */
export type RepoStatus = "ready" | "needs_token" | "needs_read_token" | "needs_ci_acceptance" | "needs_app" | "needs_approval";
/** How a link reaches its repository, as read: tokens only as metadata. */
export type RepoAccess =
  | { kind: "token"; read_token: KeyInfo; write_token: KeyInfo }
  | { kind: "github_app"; installation_id: number; account: string; approved: boolean; approved_by?: { id: string; name: string | null } | null; approved_at?: string | null };
/** What a link's form sets. A URL left out takes the forge's default; an
 * App link names none, since an App goes to the instance's forge alone, and
 * no installation: the daemon asks the forge which one holds the repository. */
export type RepoFields = {
  repo: string; base: string; repo_url?: string; forge_api_url?: string; protected_paths: string[];
  preview_url: string; preview_ready_path: string; preview_names_commit: boolean; preview_timeout_secs: number; preview_poll_secs: number;
  access: { kind: "token" } | { kind: "github_app" };
  public: boolean; allow_open_egress: boolean; reviewed_previews: boolean; wake_on_reply: boolean;
  /** A queue admin says this repository's CI may run ClaxBot's branch, with that CI's secrets and network, before anyone reviews it. A private link is not ready without it. */
  ci_accepted: boolean;
  /** The `rev` this form was read at; null for no link. */
  if_rev: string | null;
};
/** A queue's repository link, its URLs as in effect. */
export type RepoLink = Omit<RepoFields, "access" | "if_rev"> & { rev: string; repo_url: string; forge_api_url: string; access: RepoAccess; status: RepoStatus; last_ok_at: string | null; last_failure: string | null; last_failure_at: string | null; set_by: { id: string; name: string | null }; set_at: string };
/** The container runtime this Clax started with: whether one is set up, did
 * not come up, is run by a third party, holds its egress to a list, and runs
 * only what its operator names, so that a link here cannot run. */
export type RepoRuntime = { configured: boolean; third_party: boolean; egress_held: boolean; failed: boolean; operator_only?: boolean };
/** What the repository routes answer: the link, the instance's App as a
 * queue admin sees it, the runtime, and whether its operator's own
 * configuration names this queue, in which case that is what runs. */
export type RepoView = {
  repo: RepoLink | null;
  app: { configured: boolean; install_url: string | null; forge_api_url: string | null };
  runtime: RepoRuntime & { operator_target: boolean };
};
export const getRepo = (qid: string) => call<RepoView>(`${repoPath(qid)}`);
/** Sets the link. Changing its repository or either URL clears its tokens. */
export const putRepo = (qid: string, f: RepoFields) => call<RepoView>(`${repoPath(qid)}`, "PUT", f);
/** Removes the link and its tokens. */
export const deleteRepo = (qid: string, if_rev: string) => call<unknown>(`${repoPath(qid)}`, "DELETE", { if_rev });
/** Seals a token, which no answer gives back, for the link as read at
 * `if_rev`: 400 `link_changed` when it has moved on. */
export const putRepoToken = (qid: string, which: "read" | "write", token: string, if_rev: string) => call<RepoView>(`${repoPath(qid)}/tokens/${which}`, "PUT", { token, if_rev });
export const deleteRepoToken = (qid: string, which: "read" | "write") => call<RepoView>(`${repoPath(qid)}/tokens/${which}`, "DELETE");
type Named = { id: string; name: string | null };
/** A queue's repository link as the owner lists it: `approvable` when an
 * approval decides anything for it. */
export type RepoLine = {
  queue_id: string; queue_name: string; repo: string; repo_url: string; forge_api_url: string; access: "token" | "github_app"; installation_id: number | null; account: string | null;
  status: RepoStatus; approved: boolean; approvable: boolean; rev: string;
  public: boolean; allow_open_egress: boolean; reviewed_previews: boolean; wake_on_reply: boolean; preview_url: string; preview_ready_path: string; protected_paths: string[]; ci_accepted: boolean;
  maker: Named; admins: Named[]; set_by: Named; set_at: string;
};
/** `GET /api/admin/repos`, the owner's alone: every link, those waiting
 * first; the App (its key as `KeyInfo`, whose `last4` starts the key's
 * SHA-256); whether token links need approval; and the runtime. */
export type ReposView = {
  app: { app_id: number; slug: string; api_url: string; install_url: string; key: KeyInfo; set_by: Named; set_at: string } | null;
  links: RepoLine[];
  token_approval: boolean;
  kek: { set: boolean };
  runtime: RepoView["runtime"];
};
export const getRepos = () => call<ReposView>("/api/admin/repos");
/** Sets the App, and with `private_key` its key. Moving it to another forge or ID clears the key it had and voids every App approval. */
export const putGithubApp = (f: { app_id: number; slug: string; api_url?: string; private_key?: string }) => call<ReposView>("/api/admin/repos/app", "PUT", f);
export const deleteGithubApp = () => call<unknown>("/api/admin/repos/app", "DELETE");
/** Whether a token link needs the owner's approval too. */
export const putRepoPolicy = (token_approval: boolean) => call<ReposView>("/api/admin/repos/policy", "PUT", { token_approval });
/** Approves, or withdraws, a link as `l` shows it: 400 `link_changed` when it has moved on. */
export const approveRepoLink = (l: RepoLine, approved: boolean) => call<unknown>(`/api/admin/repos/links/${e(l.queue_id)}`, "PUT", { rev: l.rev, approved });
/** Removes a link and its tokens, on any queue. */
export const disconnectRepoLink = (qid: string) => call<unknown>(`/api/admin/repos/links/${e(qid)}`, "DELETE");
