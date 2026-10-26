import type { ReactNode } from "react";

/** A page that a downstream build adds to the dashboard. */
export interface ExtraPage {
  path: string;
  label: string;
  /** Hidden on the local dashboard, like the team pages. */
  cloudOnly?: boolean;
  render: () => ReactNode;
}

/** None here. A downstream build can replace this module with a Vite
 * alias to add its own pages to the nav and the router. */
export const extraPages: ExtraPage[] = [];
