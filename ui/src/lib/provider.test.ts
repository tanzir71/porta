import { describe, expect, it } from "vitest";

import { ipc } from "./ipc";

describe("provider IPC contract", () => {
  it("ships Cloudflare Quick as the migration-safe default", async () => {
    const [settings, profiles] = await Promise.all([
      ipc.getSettings(),
      ipc.listProviderProfiles(),
    ]);

    expect(settings.defaultProviderId).toBe("cloudflare-quick");
    expect(profiles[0]).toMatchObject({
      id: "cloudflare-quick",
      kind: "cloudflareQuick",
      builtIn: true,
      credentialConfigured: true,
    });
  });

  it("creates a custom profile and assigns it as a per-share override", async () => {
    const profile = await ipc.saveProviderProfile({
      name: "Vendor fallback",
      kind: "custom",
      executable: "/usr/local/bin/vendor-tunnel",
      arguments: ["serve", "{origin}"],
      publicUrl: "https://fallback.example.com",
      urlPattern: "READY",
      credentialEnv: "VENDOR_TOKEN",
      forwardedIpHeader: "X-Forwarded-For",
      credential: "test-token",
    });
    const share = await ipc.createShare({
      kind: "folder",
      path: "/Users/example/Provider Test",
      providerId: profile.id,
      startNow: false,
    });

    expect(profile).toMatchObject({
      kind: "custom",
      credentialConfigured: true,
    });
    expect(share.providerId).toBe(profile.id);
    await expect(ipc.testProvider(profile.id)).resolves.toMatchObject({
      url: "https://fallback.example.com",
    });
  });
});
