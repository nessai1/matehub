"use client";

import { useState } from "react";
import {
  FileIcon,
  DownloadIcon,
  FileTextIcon,
  FileArchiveIcon,
  Loader2,
} from "lucide-react";
import { type Attachment, attachmentKind } from "@matehub/sdk-chat";
import { ImageLightbox } from "./image-lightbox";
import { VideoPlayer } from "./video-player";

interface Props {
  attachments: Attachment[];
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function DocumentIcon({ contentType }: { contentType: string }) {
  if (contentType.includes("pdf")) return <FileTextIcon className="h-5 w-5 text-red-500" />;
  if (contentType.includes("zip")) return <FileArchiveIcon className="h-5 w-5 text-amber-500" />;
  return <FileIcon className="h-5 w-5 text-muted-foreground" />;
}

export function MessageAttachments({ attachments }: Props) {
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);

  if (attachments.length === 0) return null;

  // Gallery of all images in this message
  const imageAttachments = attachments.filter((a) => attachmentKind(a) === "image");
  const gallery = imageAttachments.map((a) => ({ url: a.url, alt: a.name }));

  return (
    <>
      <div className="mt-2 flex flex-col gap-2">
        {attachments.map((a) => {
          const kind = attachmentKind(a);
          const key = a.id || a.url;

          if (kind === "image") {
            // Clamp thumbnail to reasonable size
            const maxW = 400;
            const maxH = 300;
            let w = a.width ?? maxW;
            let h = a.height ?? maxH;
            if (w > maxW) {
              h = (h * maxW) / w;
              w = maxW;
            }
            if (h > maxH) {
              w = (w * maxH) / h;
              h = maxH;
            }
            const galleryIndex = imageAttachments.indexOf(a);
            return (
              <button
                key={key}
                onClick={() => setLightboxIndex(galleryIndex)}
                className="w-fit overflow-hidden rounded-lg border border-border/50 transition-opacity hover:opacity-90"
                style={{ maxWidth: w, maxHeight: h }}
              >
                <img
                  src={a.url}
                  alt={a.name}
                  loading="lazy"
                  className="block"
                  style={{
                    width: w,
                    height: h,
                    objectFit: "cover",
                  }}
                />
              </button>
            );
          }

          if (kind === "video") {
            // Transcoding state: show spinner placeholder
            if (a.status === "transcoding") {
              return (
                <div
                  key={key}
                  className="flex aspect-video max-w-md items-center justify-center rounded-lg border border-border/50 bg-muted/30"
                  style={{ width: 400 }}
                >
                  <div className="flex flex-col items-center gap-2 text-muted-foreground">
                    <Loader2 className="h-6 w-6 animate-spin" />
                    <span className="text-xs">Converting video...</span>
                  </div>
                </div>
              );
            }
            if (a.status === "failed") {
              return (
                <div
                  key={key}
                  className="flex items-center gap-2 rounded-lg border border-red-900/30 bg-red-950/20 px-3 py-2 text-sm text-red-400"
                >
                  <FileIcon className="h-4 w-4" />
                  <span>Video conversion failed: {a.name}</span>
                </div>
              );
            }
            return (
              <div key={key} className="max-w-md">
                <VideoPlayer
                  url={a.url}
                  poster={a.thumb_url}
                  width={a.width}
                  height={a.height}
                />
              </div>
            );
          }

          if (kind === "audio") {
            return (
              <audio
                key={key}
                src={a.url}
                controls
                preload="metadata"
                className="max-w-md"
              />
            );
          }

          // Document
          return (
            <a
              key={key}
              href={a.url}
              download={a.name}
              target="_blank"
              rel="noopener"
              className="flex w-fit items-center gap-3 rounded-lg border border-border/50 bg-muted/30 px-3 py-2 text-sm transition-colors hover:bg-muted"
            >
              <DocumentIcon contentType={a.content_type} />
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium">{a.name}</div>
                <div className="text-xs text-muted-foreground">
                  {formatSize(a.size)}
                </div>
              </div>
              <DownloadIcon className="h-4 w-4 text-muted-foreground" />
            </a>
          );
        })}
      </div>

      {lightboxIndex !== null && gallery.length > 0 && (
        <ImageLightbox
          images={gallery}
          initialIndex={lightboxIndex}
          onClose={() => setLightboxIndex(null)}
        />
      )}
    </>
  );
}
