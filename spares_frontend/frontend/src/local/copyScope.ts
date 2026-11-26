/** Storage and transport names belong to the account that opened this document. */
export const credentialsKey = "spares_credentials";

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
