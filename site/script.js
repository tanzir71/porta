(() => {
  const releaseAsset = "Porta_1.0.0_aarch64.dmg";
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
    const downloadUrl = `${releasePageUrl}/download/${releaseAsset}`;

    document.querySelectorAll("[data-repo-link]").forEach((link) => {
      link.href = repositoryUrl;
    });
    document.querySelectorAll("[data-release-page]").forEach((link) => {
      link.href = releasePageUrl;
    });
    document.querySelectorAll("[data-release-download]").forEach((link) => {
      link.href = downloadUrl;
    });
  }

  document.querySelectorAll("[data-year]").forEach((node) => {
    node.textContent = String(new Date().getFullYear());
  });

  const header = document.querySelector("[data-header]");
  const syncHeader = () => header?.classList.toggle("scrolled", window.scrollY > 8);
  syncHeader();
  window.addEventListener("scroll", syncHeader, { passive: true });

  const copyButton = document.querySelector("[data-copy-checksum]");
  const checksumNode = document.querySelector("#checksum-value");
  const checksum = checksumNode?.textContent?.trim();
  copyButton?.addEventListener("click", async () => {
    if (!checksum) return;

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
})();
