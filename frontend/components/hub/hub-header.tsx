"use client";

import { useState } from "react";
import { useSearchParams } from "next/navigation";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Separator } from "@/components/ui/separator";

interface User {
  username: string;
  displayName: string;
  role: string;
  status: "online" | "idle" | "dnd" | "offline";
}

const DEV_USERS: Record<string, User> = {
  alice: { username: "alice", displayName: "Alice", role: "admin", status: "online" },
  bob: { username: "bob", displayName: "Bob", role: "member", status: "online" },
  charlie: { username: "charlie", displayName: "Charlie", role: "member", status: "online" },
};

const statusColors: Record<User["status"], string> = {
  online: "bg-emerald-500",
  idle: "bg-amber-500",
  dnd: "bg-red-500",
  offline: "bg-zinc-400",
};

const statusLabels: Record<User["status"], string> = {
  online: "Online",
  idle: "Idle",
  dnd: "Do Not Disturb",
  offline: "Offline",
};

export function HubHeader() {
  const [open, setOpen] = useState(false);
  const searchParams = useSearchParams();
  const devUser = searchParams.get("user") ?? "alice";
  const currentUser = DEV_USERS[devUser] ?? DEV_USERS.alice;

  return (
    <header className="flex h-12 shrink-0 items-center border-b bg-card px-4">
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button className="group relative ml-auto flex items-center gap-2 rounded-full p-0.5 transition-colors hover:bg-accent">
            <Avatar className="h-7 w-7 cursor-pointer">
              <AvatarFallback className="bg-primary/15 text-xs font-medium text-primary">
                {currentUser.displayName.charAt(0)}
              </AvatarFallback>
            </Avatar>
            <span
              className={`absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-card ${statusColors[currentUser.status]}`}
            />
          </button>
        </PopoverTrigger>
        <PopoverContent align="end" className="w-72 p-0">
          <div className="p-4">
            <div className="flex items-center gap-3">
              <Avatar className="h-12 w-12">
                <AvatarFallback className="bg-primary/15 text-lg font-semibold text-primary">
                  {currentUser.displayName.charAt(0)}
                </AvatarFallback>
              </Avatar>
              <div>
                <div className="font-semibold">{currentUser.displayName}</div>
                <div className="text-xs text-muted-foreground">
                  @{currentUser.username}
                </div>
              </div>
            </div>
          </div>
          <Separator />
          <div className="p-3">
            <div className="flex items-center gap-2 text-sm">
              <span
                className={`h-2 w-2 rounded-full ${statusColors[currentUser.status]}`}
              />
              <span className="text-muted-foreground">
                {statusLabels[currentUser.status]}
              </span>
            </div>
            <div className="mt-2 flex items-center gap-2 text-sm">
              <span className="text-muted-foreground">Role:</span>
              <span className="rounded bg-primary/10 px-1.5 py-0.5 text-xs font-medium text-primary">
                {currentUser.role}
              </span>
            </div>
          </div>
          <Separator />
          <div className="p-1">
            <button className="w-full rounded-sm px-3 py-1.5 text-left text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground">
              Settings
            </button>
            <button className="w-full rounded-sm px-3 py-1.5 text-left text-sm text-red-500 transition-colors hover:bg-red-500/10">
              Log out
            </button>
          </div>
        </PopoverContent>
      </Popover>
    </header>
  );
}
