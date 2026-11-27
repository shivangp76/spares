import { DEMO, DEMO_CREDENTIALS } from "../demo";

/** Storage and transport names belong to the account that opened this document. */
export const credentialsKey = "spares_credentials";

// The demo's one account is logged in before the account is read below
if (DEMO && localStorage.getItem(credentialsKey) === null)
  localStorage.setItem(credentialsKey, JSON.stringify(DEMO_CREDENTIALS));

function currentAccount(): string | null {
  try {
    const id = JSON.parse(
      localStorage.getItem(credentialsKey) ?? "null",
    )?.accountId;
    return typeof id === "string" && id.length > 0 ? id : null;
  } catch {
    return null;
  }
}

export const copyAccount = currentAccount();

export function requireCopyAccount(): string {
  if (!copyAccount || currentAccount() !== copyAccount)
    throw new Error("Log in again to open this account’s offline collection.");
  return copyAccount;
}

export const accountSuffix = ":" + encodeURIComponent(copyAccount ?? "");
export const copyDirectory =
  "spares-account-" + encodeURIComponent(copyAccount ?? "");
export function copyKey(key: string): string {
  return key + accountSuffix;
}
