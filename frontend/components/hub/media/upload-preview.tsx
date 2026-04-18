"use client";

import { XIcon, FileIcon, Loader2 } from "lucide-react";
import type { PendingUpload } from "@/hooks/use-attachment-upload";

interface Props {
  uploads: PendingUpload[];
}

function isImage(file: File): boolean {
  return file.type.startsWith("image/");
}

function isVideo(file: File): boolean {
  return file.type.startsWith("video/");
}

export function UploadPreview({ uploads }: Props) {
  if (uploads.length === 0) return null;

  return (
    <div className="flex flex-wrap gap-2 border-b border-border/30 px-3 py-2">
      {uploads.map((u) => {
        const isImg = isImage(u.file);
        const isVid = isVideo(u.file);
        const isMedia = isImg || isVid;
        const isLoading = u.attachment === null && u.error === null;

        return (
          <div
            key={u.id}
            className="group relative overflow-hidden rounded-lg border border-border/50 bg-muted/30"
            style={{ width: isMedia ? 100 : 200, height: isMedia ? 100 : 60 }}
          >
            {/* Preview content */}
            {isImg && (
              <img
                src={u.previewUrl}
                alt=""
                className="h-full w-full object-cover"
              />
            )}
            {isVid && (
              <video
                src={u.previewUrl}
                className="h-full w-full object-cover"
                muted
              />
            )}
            {!isMedia && (
              <div className="flex h-full items-center gap-2 px-2">
                <FileIcon className="h-4 w-4 shrink-0 text-muted-foreground" />
                <div className="min-w-0 flex-1 truncate text-xs">
                  {u.file.name}
                </div>
              </div>
            )}

            {/* Loading overlay */}
            {isLoading && (
              <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/50 backdrop-blur-sm">
                <Loader2 className="h-5 w-5 animate-spin text-white" />
                {u.progress !== null && (
                  <span className="mt-1 text-[10px] font-medium text-white">
                    {u.progress}%
                  </span>
                )}
              </div>
            )}

            {/* Error overlay */}
            {u.error && (
              <div className="absolute inset-0 flex items-center justify-center bg-red-500/80 p-2">
                <span className="text-[10px] text-white text-center">{u.error}</span>
              </div>
            )}

            {/* Cancel button */}
            <button
              onClick={u.abort}
              className="absolute top-1 right-1 flex h-5 w-5 items-center justify-center rounded-full bg-black/70 text-white backdrop-blur transition-opacity hover:bg-black/90"
              title="Remove"
            >
              <XIcon className="h-3 w-3" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
