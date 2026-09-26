    const form = document.getElementById("admin-login-form");
    if (form) {
        form.addEventListener("submit", async (e) => {
            e.preventDefault();
            const errEl = document.getElementById("login-error");
            const username = document.getElementById("username")?.value;
            const password = document.getElementById("password")?.value;

            try {
                const res = await fetch("/api/admin/login", {
                    method: "POST",
                    headers: { "Content-Type": "application/x-www-form-urlencoded" },
                    body: new URLSearchParams({ username, password }),
                });

                if (!res.ok) {
                    const err = await res.json().catch(() => ({ message: "Login failed" }));
                    if (errEl) { errEl.textContent = err.message || "Login failed"; }
                    return;
                }

                location.href = "/admin";
            } catch {
                if (errEl) { errEl.textContent = "Network error"; }
            }
        });
    }
