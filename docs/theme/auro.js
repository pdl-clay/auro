(() => {
    const copyText = async (text) => {
        if (navigator.clipboard && window.isSecureContext) {
            await navigator.clipboard.writeText(text);
            return;
        }

        const field = document.createElement("textarea");
        field.value = text;
        field.setAttribute("readonly", "");
        field.style.position = "fixed";
        field.style.opacity = "0";
        document.body.appendChild(field);
        field.select();
        document.execCommand("copy");
        field.remove();
    };

    for (const button of document.querySelectorAll("[data-copy]")) {
        button.addEventListener("click", async () => {
            const original = button.textContent;
            try {
                await copyText(button.dataset.copy);
                button.textContent = "Copied";
            } catch {
                button.textContent = "Copy failed";
            }
            window.setTimeout(() => {
                button.textContent = original;
            }, 1600);
        });
    }
})();
