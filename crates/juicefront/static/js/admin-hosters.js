    var PAGE = 50;
    var allHosters = [];
    var displayedCount = 0;
    var currentTotal = 0;

    function formatTime(sec) {
        if (!sec) return "—";
        return new Date(sec * 1000).toLocaleString();
    }

    function esc(s) {
        var d = document.createElement("div");
        d.appendChild(document.createTextNode(s == null ? "" : String(s)));
        return d.innerHTML;
    }

    function buildParams() {
        var q = document.getElementById("search-input").value.trim();
        var sort = document.getElementById("sort-select").value;
        var dir = document.getElementById("dir-select").value;
        return { q: q, sort: sort, dir: dir, offset: 0, limit: PAGE };
    }

    function renderRows(hosters) {
        var html = "";
        for (var i = 0; i < hosters.length; i++) {
            var h = hosters[i];
            var action = h.banned
                ? '<a href="#" data-unban-link data-hoster="' + esc(h.host) + '">Unban</a>'
                : '<a href="#" data-ban-link data-hoster="' + esc(h.host) + '">Ban</a>';
            html +=
                '<tr data-hoster="' + esc(h.host) + '">' +
                "<td>" + esc(h.host) + "</td>" +
                "<td nowrap>" + formatTime(h.first_seen_at) + "</td>" +
                "<td nowrap>" + formatTime(h.last_seen_at) + "</td>" +
                "<td>" + esc(h.last_status || "???") + "</td>" +
                "<td>" + esc(h.last_error || "") + "</td>" +
                "<td>" + (h.banned ? "yes" : "no") + "</td>" +
                "<td>" + esc(h.banned_reason || "") + "</td>" +
                "<td>" + esc(h.banned_by || "") + "</td>" +
                "<td nowrap>" + formatTime(h.banned_at) + "</td>" +
                "<td nowrap>" + action + "</td>" +
                "</tr>";
        }
        return html;
    }

    function updateStatus() {
        var statusEl = document.getElementById("hosters-status");
        if (!statusEl) return;
        if (currentTotal > allHosters.length) {
            statusEl.textContent = "Showing " + Math.min(displayedCount, allHosters.length) + " of " + allHosters.length + " matched (total: " + currentTotal + ")";
        } else {
            statusEl.textContent = allHosters.length + " known hoster" + (allHosters.length !== 1 ? "s" : "");
        }
    }

    function renderTable() {
        var listEl = document.getElementById("hosters-list");
        if (!listEl) return;
        var toShow = allHosters.slice(0, displayedCount);
        if (!toShow.length && !allHosters.length) {
            listEl.innerHTML = "<p>No known hosters.</p>";
            document.getElementById("hosters-status").textContent = "";
            return;
        }
        var html = '<div class="table-wrap"><table>' +
            '<thead><tr>' +
            '<th>Host</th><th>First Seen</th><th>Last Seen</th><th>Status</th><th>Error</th>' +
            '<th>Banned</th><th>Ban Reason</th><th>Banned By</th><th>Banned At</th><th>Actions</th>' +
            "</tr></thead><tbody>" +
            renderRows(toShow) +
            "</tbody></table></div>";
        if (displayedCount < allHosters.length) {
            html += '<div style="text-align:center;margin:0.75rem 0">' +
                '<button class="btn btn-primary" id="load-more-btn">Load More (' +
                displayedCount + " / " + allHosters.length + ")</button></div>";
        }
        listEl.innerHTML = html;
        var loadMoreBtn = document.getElementById("load-more-btn");
        if (loadMoreBtn) {
            loadMoreBtn.addEventListener("click", function () {
                displayedCount = Math.min(displayedCount + PAGE, allHosters.length);
                renderTable();
                updateStatus();
            });
        }
        updateStatus();
    }

    async function fetchData(params) {
        var listEl = document.getElementById("hosters-list");
        var qs = "q=" + encodeURIComponent(params.q) +
            "&sort=" + encodeURIComponent(params.sort) +
            "&dir=" + encodeURIComponent(params.dir) +
            "&offset=" + params.offset +
            "&limit=" + params.limit;
        try {
            var res = await fetch("/api/admin/hosters?" + qs);
            if (!res.ok) {
                if (res.status === 401) {
                    listEl.innerHTML = '<p>Not authenticated. <a href="/admin/login">Login</a></p>';
                } else {
                    listEl.innerHTML = "<p>Error: " + res.status + "</p>";
                }
                return null;
            }
            return await res.json();
        } catch (e) {
            listEl.innerHTML = "<p>Failed to load hosters.</p>";
            return null;
        }
    }

    async function doSearch() {
        var params = buildParams();
        allHosters = [];
        displayedCount = 0;
        currentTotal = 0;
        renderTable();
        document.getElementById("hosters-list").innerHTML = "<p>Loading...</p>";

        var result = await fetchData(params);
        if (!result) return;
        allHosters = result.items || [];
        currentTotal = result.total || allHosters.length;
        displayedCount = Math.min(PAGE, allHosters.length);
        renderTable();
    }

    function findHoster(host) {
        for (var i = 0; i < allHosters.length; i++) {
            if (allHosters[i].host === host) return allHosters[i];
        }
        return null;
    }

    async function banHoster(host) {
        var reason = prompt("Ban " + host + "?\nReason (optional):", "");
        if (reason === null) return;
        try {
            var res = await fetch("/api/admin/hosters/" + encodeURIComponent(host) + "/ban", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ reason: reason }),
            });
            if (res.status === 201 || res.ok) {
                var h = findHoster(host);
                if (h) {
                    h.banned = true;
                    h.banned_reason = reason;
                    h.banned_at = Math.floor(Date.now() / 1000);
                }
                renderTable();
                updateStatus();
            } else {
                var body = await res.text();
                alert("Ban failed: " + (body || res.status));
            }
        } catch (e) {
            alert("Ban failed: " + e.message);
        }
    }

    async function unbanHoster(host) {
        if (!confirm("Unban " + host + "?")) return;
        try {
            var res = await fetch("/api/admin/hosters/" + encodeURIComponent(host) + "/ban", { method: "DELETE" });
            if (res.status === 204 || res.ok) {
                var h = findHoster(host);
                if (h) {
                    h.banned = false;
                    h.banned_reason = "";
                    h.banned_at = null;
                }
                renderTable();
                updateStatus();
            } else {
                var body = await res.text();
                alert("Unban failed: " + (body || res.status));
            }
        } catch (e) {
            alert("Unban failed: " + e.message);
        }
    }

    async function init() {
        var listEl = document.getElementById("hosters-list");
        if (!listEl) return;

        listEl.addEventListener("click", function (e) {
            var banLink = e.target.closest("[data-ban-link]");
            if (banLink) {
                e.preventDefault();
                var host = banLink.getAttribute("data-hoster");
                if (host) banHoster(host);
                return;
            }
            var unbanLink = e.target.closest("[data-unban-link]");
            if (unbanLink) {
                e.preventDefault();
                var host2 = unbanLink.getAttribute("data-hoster");
                if (host2) unbanHoster(host2);
            }
        });

        document.getElementById("search-btn").addEventListener("click", doSearch);
        document.getElementById("search-input").addEventListener("keydown", function (e) {
            if (e.key === "Enter") doSearch();
        });
        document.getElementById("sort-select").addEventListener("change", doSearch);
        document.getElementById("dir-select").addEventListener("change", doSearch);

        await doSearch();
    }

    init();
