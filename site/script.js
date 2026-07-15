(() => {
  const releaseAssets = {
    windows: "Porta_1.2.0_x64-setup.exe",
    macos: "Porta_1.2.0_aarch64.dmg",
  };
  const preferredPlatform = /Windows/i.test(navigator.userAgent) ? "windows" : "macos";
  const configuredRepository = document.documentElement.dataset.repository ?? "";

  function inferRepository() {
    if (
      configuredRepository &&
      !configuredRepository.startsWith("__")
    ) {
      return configuredRepository;
    }

    if (window.location.hostname.endsWith(".github.io")) {
      const owner = window.location.hostname.split(".")[0];
      const project = window.location.pathname.split("/").filter(Boolean)[0];
      return project ? `${owner}/${project}` : `${owner}/${owner}.github.io`;
    }

    return "";
  }

  const repository = inferRepository();
  if (repository) {
    const repositoryUrl = `https://github.com/${repository}`;
    const releasePageUrl = `${repositoryUrl}/releases/latest`;
    const downloadUrl = (platform) =>
      `${releasePageUrl}/download/${releaseAssets[platform]}`;

    document.querySelectorAll("[data-repo-link]").forEach((link) => {
      link.href = repositoryUrl;
    });
    document.querySelectorAll("[data-release-page]").forEach((link) => {
      link.href = releasePageUrl;
    });
    document.querySelectorAll("[data-download-platform]").forEach((link) => {
      link.href = downloadUrl(link.dataset.downloadPlatform);
    });
    document.querySelectorAll("[data-primary-download]").forEach((link) => {
      link.href = downloadUrl(preferredPlatform);
    });
  }

  const primaryLabel =
    preferredPlatform === "windows" ? "Download for Windows" : "Download for macOS";
  const primaryNote =
    preferredPlatform === "windows"
      ? "Windows 10/11 · x64 · Free forever"
      : "Apple silicon · macOS · Free forever";
  document.querySelectorAll("[data-primary-label]").forEach((node) => {
    node.textContent = primaryLabel;
  });
  document.querySelectorAll("[data-primary-note]").forEach((node) => {
    node.textContent = primaryNote;
  });

  document.querySelectorAll("[data-year]").forEach((node) => {
    node.textContent = String(new Date().getFullYear());
  });

  const header = document.querySelector("[data-header]");
  const syncHeader = () => header?.classList.toggle("scrolled", window.scrollY > 8);
  syncHeader();
  window.addEventListener("scroll", syncHeader, { passive: true });

  document.querySelectorAll("[data-copy-checksum]").forEach((copyButton) => {
    const checksumNode = document.querySelector(`#${copyButton.dataset.copyChecksum}`);
    const checksum = checksumNode?.textContent?.trim();
    const ready = /^[a-f0-9]{64}$/i.test(checksum ?? "");
    copyButton.disabled = !ready;
    copyButton.addEventListener("click", async () => {
      if (!ready || !checksum) return;

      try {
        await navigator.clipboard.writeText(checksum);
        const previous = copyButton.textContent;
        copyButton.textContent = "Copied";
        window.setTimeout(() => {
          copyButton.textContent = previous;
        }, 1600);
      } catch {
        if (checksumNode) window.getSelection()?.selectAllChildren(checksumNode);
      }
    });
  });
})();
