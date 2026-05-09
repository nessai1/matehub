import { createBrowserRouter } from "react-router";
import { AuthLayout } from "@/app/(auth)/layout";
import { HubLayout } from "@/app/hub/layout";

import RootRedirect from "@/app/page";
import LoginPage from "@/app/(auth)/login/page";
import InvitePage from "@/app/(auth)/register/page";
import JoinPage from "@/app/(auth)/join/page";
import SignupPage from "@/app/(auth)/signup/page";
import SetupPage from "@/app/setup/page";
import DashboardPage from "@/app/dashboard/page";
import HubPage from "@/app/hub/page";

// App Router used implicit file-based layouts; React Router wants them
// wired explicitly. Each path either nests under a layout or stands alone.
export const router = createBrowserRouter([
  { path: "/", element: <RootRedirect /> },
  { path: "/setup", element: <SetupPage /> },
  {
    element: <AuthLayout />,
    children: [
      { path: "/login", element: <LoginPage /> },
      { path: "/invite/:token", element: <InvitePage /> },
      { path: "/join/:token", element: <JoinPage /> },
      { path: "/signup/:token", element: <SignupPage /> },
    ],
  },
  { path: "/dashboard", element: <DashboardPage /> },
  {
    path: "/hub",
    element: <HubLayout />,
    children: [{ index: true, element: <HubPage /> }],
  },
]);
