// next-themes 0.4.x declares `ThemeProviderProps extends React.PropsWithChildren`
// without a generic argument. Under React 19's tightened types this collapses
// to `unknown & { children?: ReactNode }`, which TypeScript no longer treats
// as exposing a `children` field on the resulting interface. The augmentation
// below re-adds it so JSX `<ThemeProvider>...</ThemeProvider>` type-checks.
import type * as React from "react";

declare module "next-themes" {
  interface ThemeProviderProps {
    children?: React.ReactNode;
  }
}
