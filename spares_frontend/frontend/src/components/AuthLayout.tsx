import type { ReactNode } from "react";

/**
 * The column the pages before logging in share (logging in, signing up, resetting a password):
 * a title, the page's form or message, its error, and links to the other pages.
 */
export default function AuthLayout({
  title,
  error,
  links,
  children,
}: {
  title: string;
  error?: string | null;
  links?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="login">
      <h2>{title}</h2>
      {children}
      {/* Always rendered, so a screen reader announces an error when it appears */}
      <div role="alert" className="error-text-small login-error">
        {error}
      </div>
      {links && <div className="login-links">{links}</div>}
    </div>
  );
}
