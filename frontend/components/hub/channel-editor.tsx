"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import {
  HashIcon,
  MicIcon,
  RadioIcon,
  MessageSquareIcon,
  BookOpenIcon,
  MegaphoneIcon,
  ZapIcon,
  StarIcon,
  HeartIcon,
  ImageIcon,
  CheckIcon,
  Trash2Icon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { MemberGroup } from "@/hooks/use-members";

type ChannelType = "text" | "voice" | "stage";

// ── Icon picker options ──────────────────────────

const allIcons = [
  { id: "hash", icon: HashIcon, label: "Hash", for: "text" as const },
  { id: "mic", icon: MicIcon, label: "Mic", for: "voice" as const },
  { id: "radio", icon: RadioIcon, label: "Radio", for: "voice" as const },
  { id: "chat", icon: MessageSquareIcon, label: "Chat", for: null },
  { id: "book", icon: BookOpenIcon, label: "Book", for: null },
  { id: "megaphone", icon: MegaphoneIcon, label: "Megaphone", for: null },
  { id: "zap", icon: ZapIcon, label: "Zap", for: null },
  { id: "star", icon: StarIcon, label: "Star", for: null },
  { id: "heart", icon: HeartIcon, label: "Heart", for: null },
] as const;

const availableColors = [
  "#6366f1", "#8b5cf6", "#a855f7", "#ec4899",
  "#ef4444", "#f97316", "#eab308", "#22c55e",
  "#14b8a6", "#06b6d4", "#3b82f6", "#6b7280",
];

type IconMode = "icon" | "color" | "image";

// Groups that can never be removed from channel visibility
const PROTECTED_GROUPS = ["admin", "everyone"];

// ── Types ────────────────────────────────────────

export interface ChannelEditorData {
  name: string;
  type: ChannelType;
  iconId: string;
  iconColor: string | null;
  iconImage: string | null;
  allowedGroups: string[];
  /** File to upload as icon (S3). Present when user picked an image. */
  pendingIconFile: File | null;
}

interface ChannelEditorProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  mode: "create" | "edit";
  channelType: ChannelType;
  /** True for "general" channel -- can't delete or change visibility */
  isDefault?: boolean;
  initial?: Partial<ChannelEditorData>;
  allGroups: MemberGroup[];
  onSave: (data: ChannelEditorData) => void;
  onDelete?: () => void;
}

// ── Component ────────────────────────────────────

export function ChannelEditor({
  open,
  onOpenChange,
  mode,
  channelType,
  isDefault = false,
  initial,
  allGroups,
  onSave,
  onDelete,
}: ChannelEditorProps) {
  const [name, setName] = useState("");
  const [iconId, setIconId] = useState("hash");
  const [iconColor, setIconColor] = useState<string | null>(null);
  const [iconImage, setIconImage] = useState<string | null>(null);
  const [iconMode, setIconMode] = useState<IconMode>("icon");
  const [selectedGroups, setSelectedGroups] = useState<Set<string>>(new Set());
  const [iconPickerOpen, setIconPickerOpen] = useState(false);
  const [pendingFile, setPendingFile] = useState<File | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const typeLabel = channelType === "text" ? "Text" : channelType === "voice" ? "Voice" : "Stage";
  const title = mode === "create"
    ? `Create ${typeLabel} Channel`
    : `Edit ${typeLabel} Channel`;

  // Filter icons: text channels hide voice icons, voice channels hide text icons
  const availableIcons = allIcons.filter((i) => {
    if (i.for === null) return true;
    if (channelType === "text") return i.for === "text";
    return i.for === "voice";
  });

  const SelectedIcon = allIcons.find((i) => i.id === iconId)?.icon ?? HashIcon;

  const handleImageUpload = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    setPendingFile(file);
    // Preview via data URL
    const reader = new FileReader();
    reader.onload = (ev) => {
      setIconImage(ev.target?.result as string);
      setIconMode("image");
    };
    reader.readAsDataURL(file);
  }, []);

  const toggleGroup = useCallback((groupId: string, groupName: string) => {
    // Can't remove protected groups
    if (PROTECTED_GROUPS.includes(groupName.toLowerCase())) return;
    setSelectedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(groupId)) next.delete(groupId);
      else next.add(groupId);
      return next;
    });
  }, []);

  const handleSave = useCallback(() => {
    if (!name.trim()) return;
    onSave({
      name: name.trim(),
      type: channelType,
      iconId,
      iconColor: iconMode === "color" ? iconColor : null,
      iconImage: iconMode === "image" && !pendingFile ? iconImage : null,
      allowedGroups: Array.from(selectedGroups),
      pendingIconFile: pendingFile,
    });
    setPendingFile(null);
    onOpenChange(false);
  }, [name, channelType, iconId, iconMode, iconColor, iconImage, pendingFile, selectedGroups, onSave, onOpenChange]);

  // Reset state when dialog opens (useEffect sees updated props after batch setState)
  useEffect(() => {
    if (!open) return;
    const defIcon = channelType === "text" ? "hash" : "mic";
    if (mode === "edit" && initial) {
      setName(initial.name ?? "");
      setIconId(initial.iconId ?? defIcon);
      setIconColor(initial.iconColor ?? null);
      setIconImage(initial.iconImage ?? null);
      setIconMode(initial.iconImage ? "image" : initial.iconColor ? "color" : "icon");
      setSelectedGroups(new Set(initial.allowedGroups ?? allGroups.map((g) => g.id)));
    } else {
      setName("");
      setIconId(defIcon);
      setIconColor(null);
      setIconImage(null);
      setIconMode("icon");
      setSelectedGroups(new Set(allGroups.map((g) => g.id)));
    }
    setIconPickerOpen(false);
    setPendingFile(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // ── Render channel icon preview ──
  const renderIconPreview = () => {
    if (iconMode === "image" && iconImage) {
      return (
        <div className="h-14 w-14 overflow-hidden rounded-xl">
          <img src={iconImage} alt="" className="h-full w-full object-cover" />
        </div>
      );
    }
    if (iconMode === "color" && iconColor) {
      return (
        <div
          className="flex h-14 w-14 items-center justify-center rounded-xl"
          style={{ backgroundColor: iconColor }}
        >
          <SelectedIcon className="h-7 w-7 text-white" />
        </div>
      );
    }
    return (
      <div className="flex h-14 w-14 items-center justify-center rounded-xl bg-muted">
        <SelectedIcon className="h-7 w-7 text-muted-foreground" />
      </div>
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>

        <div className="flex flex-col items-center gap-5 py-4">
          {/* Icon picker trigger */}
          <Popover open={iconPickerOpen} onOpenChange={setIconPickerOpen}>
            <PopoverTrigger asChild>
              <button className="group relative cursor-pointer rounded-xl transition-transform hover:scale-105">
                {renderIconPreview()}
                <div className="absolute inset-0 flex items-center justify-center rounded-xl bg-black/40 opacity-0 transition-opacity group-hover:opacity-100">
                  <ImageIcon className="h-5 w-5 text-white" />
                </div>
              </button>
            </PopoverTrigger>
            <PopoverContent side="bottom" className="w-64 p-3">
              {/* Mode tabs */}
              <div className="mb-3 flex gap-1 rounded-lg bg-muted p-0.5">
                {(["icon", "color", "image"] as const).map((m) => (
                  <button
                    key={m}
                    onClick={() => setIconMode(m)}
                    className={cn(
                      "flex-1 rounded-md px-2 py-1 text-xs font-medium transition-colors",
                      iconMode === m
                        ? "bg-background text-foreground shadow-sm"
                        : "text-muted-foreground hover:text-foreground",
                    )}
                  >
                    {m === "icon" ? "Icon" : m === "color" ? "Color" : "Image"}
                  </button>
                ))}
              </div>

              {/* Icon grid */}
              {iconMode === "icon" && (
                <div className="grid grid-cols-5 gap-1">
                  {availableIcons.map((item) => {
                    const I = item.icon;
                    return (
                      <button
                        key={item.id}
                        onClick={() => setIconId(item.id)}
                        className={cn(
                          "flex h-10 w-full items-center justify-center rounded-lg transition-colors",
                          iconId === item.id
                            ? "bg-primary/15 text-primary"
                            : "text-muted-foreground hover:bg-muted",
                        )}
                        title={item.label}
                      >
                        <I className="h-4 w-4" />
                      </button>
                    );
                  })}
                </div>
              )}

              {/* Color grid */}
              {iconMode === "color" && (
                <div className="grid grid-cols-6 gap-1.5">
                  {availableColors.map((color) => (
                    <button
                      key={color}
                      onClick={() => setIconColor(color)}
                      className={cn(
                        "relative h-8 w-full rounded-lg transition-transform hover:scale-110",
                      )}
                      style={{ backgroundColor: color }}
                    >
                      {iconColor === color && (
                        <CheckIcon className="absolute inset-0 m-auto h-4 w-4 text-white" />
                      )}
                    </button>
                  ))}
                </div>
              )}

              {/* Image upload */}
              {iconMode === "image" && (
                <div className="flex flex-col items-center gap-2">
                  {iconImage && (
                    <div className="h-16 w-16 overflow-hidden rounded-lg">
                      <img src={iconImage} alt="" className="h-full w-full object-cover" />
                    </div>
                  )}
                  <Button
                    variant="outline"
                    size="sm"
                    className="w-full text-xs"
                    onClick={() => fileInputRef.current?.click()}
                  >
                    <ImageIcon className="mr-1.5 h-3.5 w-3.5" />
                    {iconImage ? "Change image" : "Upload image"}
                  </Button>
                  <input
                    ref={fileInputRef}
                    type="file"
                    accept="image/png,image/jpeg,image/webp,image/gif"
                    className="hidden"
                    onChange={handleImageUpload}
                  />
                </div>
              )}
            </PopoverContent>
          </Popover>

          {/* Channel name */}
          <div className="w-full space-y-1.5">
            <label className="text-xs font-medium text-muted-foreground">Channel name</label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={channelType === "text" ? "general" : "voice-chat"}
              autoFocus
            />
          </div>

          {/* Group access (hidden for default channel) */}
          {!isDefault && <div className="w-full space-y-1.5">
            <label className="text-xs font-medium text-muted-foreground">Visible to groups</label>
            <div className="flex flex-wrap gap-1.5">
              {allGroups.map((g) => {
                const selected = selectedGroups.has(g.id);
                const isProtected = PROTECTED_GROUPS.includes(g.name.toLowerCase());
                return (
                  <button
                    key={g.id}
                    onClick={() => toggleGroup(g.id, g.name)}
                    disabled={isProtected}
                    className="outline-none"
                  >
                    <Badge
                      variant={selected ? "default" : "outline"}
                      className={cn(
                        "cursor-pointer text-xs transition-colors",
                        !selected && "opacity-40",
                        isProtected && "cursor-not-allowed",
                      )}
                      style={
                        g.color && selected
                          ? {
                              backgroundColor: `${g.color}20`,
                              borderColor: `${g.color}40`,
                              color: g.color,
                            }
                          : undefined
                      }
                    >
                      {g.name}
                      {isProtected && selected && (
                        <CheckIcon className="ml-1 h-2.5 w-2.5 opacity-50" />
                      )}
                    </Badge>
                  </button>
                );
              })}
            </div>
          </div>}
        </div>

        <DialogFooter className="flex-row">
          {mode === "edit" && !isDefault && onDelete && (
            <Button
              variant="outline"
              size="sm"
              className="mr-auto gap-1.5 border-red-200 text-red-500 hover:bg-red-50 dark:border-red-900 dark:text-red-400 dark:hover:bg-red-950"
              onClick={() => {
                onDelete();
                onOpenChange(false);
              }}
            >
              <Trash2Icon className="h-3.5 w-3.5" />
              Delete
            </Button>
          )}
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={handleSave} disabled={!name.trim()}>
            {mode === "create" ? "Create" : "Save"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
