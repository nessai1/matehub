import { useEffect, useState } from "react"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar"
import {
  ChevronsUpDownIcon,
  SettingsIcon,
  UsersIcon,
  ShieldIcon,
  GlobeIcon,
  MessageSquareIcon,
} from "lucide-react"
import {
  Avatar,
  AvatarFallback,
  AvatarImage,
} from "@/components/ui/avatar"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { GroupsRolesDialog } from "@/components/hub/groups-roles-dialog"
import { HubSettingsDialog } from "@/components/hub/hub-settings-dialog"
import { useMembers } from "@/hooks/use-members"
import { usePermissions, P } from "@/hooks/use-permissions"
import { useAuth } from "@/lib/auth"
import { t } from "@/i18n"

interface Hub {
  id: string
  name: string
  slug: string
  plan: string
  avatarUrl: string | null
  description: string | null
}

interface ServiceInfo {
  name: string
  version: string | null
  status: "up" | "down"
}

export function HubSwitcher({
  currentHub,
  otherHubs,
}: {
  currentHub: Hub
  otherHubs: Hub[]
}) {
  const { isMobile } = useSidebar()
  const { refetch: refetchMembers } = useMembers()
  const { session } = useAuth()
  const { perms, has } = usePermissions()
  const [rolesOpen, setRolesOpen] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [menuOpen, setMenuOpen] = useState(false)
  const [services, setServices] = useState<ServiceInfo[] | null>(null)
  const [feedbackOpen, setFeedbackOpen] = useState(false)
  const [feedbackEmail, setFeedbackEmail] = useState("")
  const [feedbackText, setFeedbackText] = useState("")
  const [feedbackSending, setFeedbackSending] = useState(false)
  const [feedbackSent, setFeedbackSent] = useState(false)

  // Lazy-load service versions: only when the dropdown opens, and only
  // once per session — versions don't change between page loads, and
  // the aggregator does N HTTP calls server-side so caching matters.
  useEffect(() => {
    if (!menuOpen || services !== null || !session?.token) return
    let cancelled = false
    fetch(`/api/hub/v1/services/versions`, {
      headers: { Authorization: `Bearer ${session.token}` },
    })
      .then((r) => (r.ok ? r.json() : []))
      .then((data: ServiceInfo[]) => {
        if (!cancelled) setServices(data)
      })
      .catch(() => {
        if (!cancelled) setServices([])
      })
    return () => {
      cancelled = true
    }
  }, [menuOpen, services, session?.token])

  return (
    <>
      <SidebarMenu>
        <SidebarMenuItem>
          <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
            <DropdownMenuTrigger asChild>
              <SidebarMenuButton
                size="lg"
                className="data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground"
              >
                <Avatar className="aspect-square size-8 rounded-md">
                  {currentHub.avatarUrl && (
                    <AvatarImage src={currentHub.avatarUrl} className="object-cover" />
                  )}
                  <AvatarFallback className="rounded-md bg-sidebar-primary text-sidebar-primary-foreground">
                    {currentHub.name?.charAt(0).toUpperCase() ?? <UsersIcon className="size-4" />}
                  </AvatarFallback>
                </Avatar>
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-medium">{currentHub.name}</span>
                  <span className="w-fit rounded bg-blue-600/20 px-1.5 py-0.5 text-[10px] font-medium text-blue-400">
                    pre-alpha {import.meta.env.VITE_APP_VERSION || "v0.0.1"}
                  </span>
                </div>
                <ChevronsUpDownIcon className="ml-auto" />
              </SidebarMenuButton>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              className="w-(--radix-dropdown-menu-trigger-width) min-w-56 rounded-lg"
              align="start"
              side={isMobile ? "bottom" : "right"}
              sideOffset={4}
            >
              <DropdownMenuLabel className="text-xs text-muted-foreground">
                App
              </DropdownMenuLabel>
              <DropdownMenuItem className="gap-2" onSelect={() => window.open("https://matehub.io", "_blank")}>
                <GlobeIcon className="size-4 text-muted-foreground" />
                matehub.io
              </DropdownMenuItem>
              <DropdownMenuItem className="gap-2" onSelect={() => {
                setFeedbackEmail(session?.username ? `${session.username}@matehub.io` : "")
                setFeedbackText("")
                setFeedbackSent(false)
                setFeedbackOpen(true)
              }}>
                <MessageSquareIcon className="size-4 text-muted-foreground" />
                {t("Send Feedback")}
              </DropdownMenuItem>
              {(perms?.is_admin || has(P.MANAGE_ROLES)) && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuLabel className="text-xs text-muted-foreground">
                    {t("Hub Settings")}
                  </DropdownMenuLabel>
                  {perms?.is_admin && (
                    <DropdownMenuItem
                      className="gap-2"
                      onSelect={() => setSettingsOpen(true)}
                    >
                      <SettingsIcon className="size-4 text-muted-foreground" />
                      {t("General")}
                    </DropdownMenuItem>
                  )}
                  {has(P.MANAGE_ROLES) && (
                    <DropdownMenuItem className="gap-2" onSelect={() => setRolesOpen(true)}>
                      <ShieldIcon className="size-4 text-muted-foreground" />
                      {t("Groups & Roles")}
                    </DropdownMenuItem>
                  )}
                </>
              )}

              <DropdownMenuSeparator />
              <DropdownMenuLabel className="text-xs text-muted-foreground">
                {t("Services")}
              </DropdownMenuLabel>
              {services === null ? (
                <div className="px-2 py-1.5 text-xs text-muted-foreground">
                  {t("Loading...")}
                </div>
              ) : services.length === 0 ? (
                <div className="px-2 py-1.5 text-xs text-muted-foreground">
                  {t("No services reachable")}
                </div>
              ) : (
                services.map((s) => (
                  <div
                    key={s.name}
                    className="flex items-center gap-2 px-2 py-1 text-xs"
                  >
                    <span
                      className={
                        s.status === "up"
                          ? "size-1.5 rounded-full bg-emerald-500"
                          : "size-1.5 rounded-full bg-zinc-500"
                      }
                      aria-hidden
                    />
                    <span className="font-medium">{s.name}</span>
                    <span className="ml-auto font-mono text-[10px] text-muted-foreground">
                      {s.version ?? "—"}
                    </span>
                  </div>
                ))
              )}
              {otherHubs.length > 0 && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuLabel className="text-xs text-muted-foreground">
                    {t("Other Hubs")}
                  </DropdownMenuLabel>
                  {otherHubs.map((hub) => (
                    <DropdownMenuItem key={hub.id} className="gap-2 p-2">
                      <div className="flex size-6 items-center justify-center rounded-md border text-xs font-medium">
                        {hub.name.charAt(0).toUpperCase()}
                      </div>
                      {hub.name}
                    </DropdownMenuItem>
                  ))}
                </>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        </SidebarMenuItem>
      </SidebarMenu>

      <GroupsRolesDialog open={rolesOpen} onOpenChange={setRolesOpen} onGroupsChanged={refetchMembers} />
      <HubSettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />

      <Dialog open={feedbackOpen} onOpenChange={(v: boolean) => setFeedbackOpen(v)}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("Send Feedback")}</DialogTitle>
          </DialogHeader>

          {feedbackSent ? (
            <div className="py-8 text-center">
              <p className="text-sm font-medium">{t("Thank you!")}</p>
              <p className="mt-1 text-xs text-muted-foreground">{t("Your feedback has been sent.")}</p>
            </div>
          ) : (
            <div className="flex flex-col gap-4 py-2">
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-muted-foreground">{t("Email")}</label>
                <Input
                  type="email"
                  value={feedbackEmail}
                  onChange={(e) => setFeedbackEmail(e.target.value)}
                  placeholder="you@example.com"
                />
              </div>
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-muted-foreground">{t("Message")}</label>
                <textarea
                  value={feedbackText}
                  onChange={(e) => setFeedbackText(e.target.value)}
                  placeholder={t("What's on your mind?")}
                  rows={4}
                  className="w-full resize-none rounded-md border bg-transparent px-3 py-2 text-sm outline-none placeholder:text-muted-foreground focus:ring-1 focus:ring-primary"
                />
              </div>
            </div>
          )}

          <DialogFooter>
            {feedbackSent ? (
              <Button onClick={() => setFeedbackOpen(false)}>{t("Close")}</Button>
            ) : (
              <>
                <Button variant="outline" onClick={() => setFeedbackOpen(false)}>
                  {t("Cancel")}
                </Button>
                <Button
                  disabled={!feedbackEmail.trim() || !feedbackText.trim() || feedbackSending}
                  onClick={async () => {
                    setFeedbackSending(true)
                    try {
                      // TODO: POST to feedback API
                      await new Promise((r) => setTimeout(r, 500))
                      setFeedbackSent(true)
                    } finally {
                      setFeedbackSending(false)
                    }
                  }}
                >
                  {feedbackSending ? t("Sending...") : t("Send")}
                </Button>
              </>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
