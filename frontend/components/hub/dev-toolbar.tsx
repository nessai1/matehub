"use client";

import { useSearchParams, usePathname } from "next/navigation";
import { Button } from "@/components/ui/button";

const DEV_USERS = ["alice", "bob", "charlie"] as const;

export function DevToolbar() {
  const searchParams = useSearchParams();
  const pathname = usePathname();
  const currentUser = searchParams.get("user") ?? "alice";

  const switchUser = (user: string) => {
    if (user === currentUser) return;
    const params = new URLSearchParams(searchParams.toString());
    params.set("user", user);
    // Full reload to ensure all components pick up the new user
    window.location.href = `${pathname}?${params.toString()}`;
  };

  const openAsUser = (user: string) => {
    const params = new URLSearchParams(searchParams.toString());
    params.set("user", user);
    window.open(`${pathname}?${params.toString()}`, "_blank");
  };

  // Pick a different user for "Open in new tab"
  const otherUser = DEV_USERS.find((u) => u !== currentUser) ?? "bob";

  return (
    <div className="flex h-8 items-center gap-3 border-b bg-amber-500/10 px-4 text-xs">
      <span className="font-mono font-medium text-amber-600">DEV MODE</span>
      <span className="text-muted-foreground">|</span>

      <span className="text-muted-foreground">User:</span>
      {DEV_USERS.map((user) => (
        <Button
          key={user}
          variant={currentUser === user ? "default" : "ghost"}
          size="sm"
          className="h-5 px-2 text-xs"
          onClick={() => switchUser(user)}
        >
          {user}
        </Button>
      ))}

      <span className="text-muted-foreground">|</span>

      <Button
        variant="ghost"
        size="sm"
        className="h-5 px-2 text-xs"
        onClick={() => openAsUser(otherUser)}
      >
        Open as {otherUser} in new tab
      </Button>

      <span className="ml-auto font-mono text-muted-foreground">
        hub: dev-hub-000 | user: {currentUser}
      </span>
    </div>
  );
}
