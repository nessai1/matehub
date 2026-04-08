import type { VideoClientOptions } from "./types";

/**
 * MateHub Video SDK client.
 *
 * Handles WebRTC signaling, media track management,
 * and session lifecycle for video/voice channels.
 *
 * Usage:
 *   const client = new VideoClient({ wsUrl, token });
 *   await client.connect();
 *   await client.publishCamera();
 */
export class VideoClient {
  constructor(_options: VideoClientOptions) {
    // TODO: implement
  }
}
