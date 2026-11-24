// A decision this page made that the daemon holds for confirmation
// (docs/contract.md, "Passkeys"): while passkeys are required, the owner
// confirms it here, with their passkey, rather than on its card. Loaded by
// `heldFetch` when a decision is held, so no eager entry carries it.
import type { Held } from "../held";
import { answerConfirmation, confirmations } from "./confirm.svelte";
import { assertFor, passkeySettings, unavailableHere } from "./passkey";

/** Confirms `held` with the owner's passkey: true once approved, false
 * where no passkey confirms here (not required, or this browser offers
 * none), leaving it to its card. A passkey not used declines it, so no
 * card is left waiting on a call nobody makes again, and throws. */
export async function confirmHere(held: Held): Promise<boolean> {
  const s = await passkeySettings(true);
  if (!s?.required || unavailableHere()) return false;
  let assertion;
  try {
    assertion = await assertFor({ purpose: "confirm", confirmation_id: held.id });
  } catch {
    const c = await answerConfirmation(held.id, false).catch(() => null);
    if (c) confirmations().take(c);
    throw new Error("Not done: confirming it takes your passkey, and none was used.");
  }
  const c = await answerConfirmation(held.id, true, assertion);
  confirmations().take(c);
  return c.status === "approved";
}
