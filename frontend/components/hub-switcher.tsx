import { useState } from "react"
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
  LinkIcon,
  UsersIcon,
  ShieldIcon,
  GlobeIcon,
  MessageSquareIcon,
} from "lucide-react"
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
import { useMembers } from "@/hooks/use-members"
import { usePermissions, P } from "@/hooks/use-permissions"
import { useAuth } from "@/lib/auth"

interface Hub {
  id: string
  name: string
  slug: string
  plan: string
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
  const [feedbackOpen, setFeedbackOpen] = useState(false)
  const [feedbackEmail, setFeedbackEmail] = useState("")
  const [feedbackText, setFeedbackText] = useState("")
  const [feedbackSending, setFeedbackSending] = useState(false)
  const [feedbackSent, setFeedbackSent] = useState(false)

  return (
    <>
      <SidebarMenu>
        <SidebarMenuItem>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <SidebarMenuButton
                size="lg"
                className="data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground"
              >
                <div className="flex aspect-square size-8 items-center justify-center rounded-md bg-sidebar-primary text-sidebar-primary-foreground">
                  <UsersIcon className="size-4" />
                </div>
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
                Send Feedback
              </DropdownMenuItem>
              {(perms?.is_admin || has(P.MANAGE_ROLES) || has(P.INVITE_PERMANENT) || has(P.CREATE_TEMP_LINKS)) && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuLabel className="text-xs text-muted-foreground">
                    Hub Settings
                  </DropdownMenuLabel>
                  {perms?.is_admin && (
                    <DropdownMenuItem className="gap-2">
                      <SettingsIcon className="size-4 text-muted-foreground" />
                      General
                    </DropdownMenuItem>
                  )}
                  {has(P.MANAGE_ROLES) && (
                    <DropdownMenuItem className="gap-2" onSelect={() => setRolesOpen(true)}>
                      <ShieldIcon className="size-4 text-muted-foreground" />
                      Groups & Roles
                    </DropdownMenuItem>
                  )}
                  {(has(P.INVITE_PERMANENT) || has(P.CREATE_TEMP_LINKS)) && (
                    <DropdownMenuItem className="gap-2">
                      <LinkIcon className="size-4 text-muted-foreground" />
                      Invite Links
                    </DropdownMenuItem>
                  )}
                </>
              )}
              {otherHubs.length > 0 && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuLabel className="text-xs text-muted-foreground">
                    Other Hubs
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

      <Dialog open={feedbackOpen} onOpenChange={(v: boolean) => setFeedbackOpen(v)}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>Send Feedback</DialogTitle>
          </DialogHeader>

          {feedbackSent ? (
            <div className="py-8 text-center">
              <p className="text-sm font-medium">Thank you!</p>
              <p className="mt-1 text-xs text-muted-foreground">Your feedback has been sent.</p>
            </div>
          ) : (
            <div className="flex flex-col gap-4 py-2">
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-muted-foreground">Email</label>
                <Input
                  type="email"
                  value={feedbackEmail}
                  onChange={(e) => setFeedbackEmail(e.target.value)}
                  placeholder="you@example.com"
                />
              </div>
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-muted-foreground">Message</label>
                <textarea
                  value={feedbackText}
                  onChange={(e) => setFeedbackText(e.target.value)}
                  placeholder="What's on your mind?"
                  rows={4}
                  className="w-full resize-none rounded-md border bg-transparent px-3 py-2 text-sm outline-none placeholder:text-muted-foreground focus:ring-1 focus:ring-primary"
                />
              </div>
            </div>
          )}

          <DialogFooter>
            {feedbackSent ? (
              <Button onClick={() => setFeedbackOpen(false)}>Close</Button>
            ) : (
              <>
                <Button variant="outline" onClick={() => setFeedbackOpen(false)}>
                  Cancel
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
                  {feedbackSending ? "Sending..." : "Send"}
                </Button>
              </>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
