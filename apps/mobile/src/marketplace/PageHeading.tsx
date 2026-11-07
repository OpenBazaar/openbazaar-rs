import type { ReactNode } from "react";

export function PageHeading({ title, description, action }: { title: string; description?: string; action?: ReactNode }) {
  return <header className="ob-page-heading"><div><h1>{title}</h1>{description && <p>{description}</p>}</div>{action && <div className="ob-page-heading-action">{action}</div>}</header>;
}
