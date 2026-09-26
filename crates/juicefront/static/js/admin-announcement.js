    async function loadAnnouncement() {
        try {
            var res = await fetch("/api/admin/announcement");
            if (!res.ok) return;
            var data = await res.json();
            if (data && data.message !== undefined) {
                document.getElementById("ann-message").value = data.message;
                document.getElementById("ann-link").value = data.link_url || "";
                document.getElementById("ann-mode").value = data.mode || "warning";
                document.getElementById("ann-active").checked = data.is_active;
            }
        } catch {}
    }

    var form = document.getElementById("announcement-form");
    if (form) {
        form.addEventListener("submit", async function (e) {
            e.preventDefault();
            var statusEl = document.getElementById("ann-status");
            statusEl.textContent = "Saving...";
            try {
                var res = await fetch("/api/admin/announcement", {
                    method: "PUT",
                    headers: { "Content-Type": "application/json" },
                    body: JSON.stringify({
                        message: document.getElementById("ann-message").value,
                        link_url: document.getElementById("ann-link").value,
                        mode: document.getElementById("ann-mode").value,
                        is_active: document.getElementById("ann-active").checked,
                    }),
                });
                if (res.ok) {
                    statusEl.textContent = "Saved!";
                    loadAnnouncement();
                } else {
                    statusEl.textContent = "Failed: " + res.status;
                }
            } catch (err) {
                statusEl.textContent = "Error: " + err.message;
            }
        });
    }

    loadAnnouncement();
