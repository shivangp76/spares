import { useEffect } from "react";

const APP_NAME = "Spares";

/**
 * A page's title, with what the page is for under it when there is a `description`. Also names the
 * browser tab and history entry after the page, as "Notes · Spares".
 */
export default function PageTitle({
  children,
  description,
  style,
}: {
  children: string;
  description?: React.ReactNode;
  style?: React.CSSProperties;
}) {
  useEffect(() => {
    document.title = `${children} · ${APP_NAME}`;
    return () => {
      document.title = APP_NAME;
    };
  }, [children]);

  return (
    <>
      <h2 className="page-title" style={style}>
        {children}
      </h2>
      {description && <p className="page-description">{description}</p>}
    </>
  );
}
