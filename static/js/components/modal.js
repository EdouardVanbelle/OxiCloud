import { i18n } from '../core/i18n.js';
import { replaceIconsInElement } from '../core/icons.js';

/**
 * Modal System for OxiCloud
 * Provides modern, styled modals to replace browser prompts/alerts
 */

const Modal = {
    // Modal element references
    /** @private @type {HTMLElement | null} */
    overlay: null,
    /** @private @type {HTMLElement | null} */
    icon: null,
    /** @private @type {HTMLElement | null} */
    title: null,
    /** @private @type {HTMLElement | null} */
    label: null,
    /** @private @type {HTMLInputElement | null} */
    input: null,
    /** @private @type {HTMLButtonElement | null} */
    cancelBtn: null,
    /** @private @type {HTMLButtonElement | null} */
    confirmBtn: null,
    /** @private @type {HTMLButtonElement | null} */
    closeBtn: null,

    // Current callback
    /** @private @type {Function | null} */
    onConfirm: null,
    /** @private @type {Function | null} */
    onCancel: null,

    /** @private @type {((value: string) => Promise<void>) | null} */
    _action: null,

    /** @private @type {HTMLElement | null} */
    errorEl: null,

    // Rename mode: select only name without extension
    _selectNameOnly: false,

    // Panel mode — openPanel() sets this; skips input-focus logic
    /** @private */
    _panelMode: false,

    // Saved modal-body innerHTML to restore when a panel closes
    /** @private */
    _savedBodyHTML: '',

    // Element focused before the modal opened — focus returns here on close.
    /** @private @type {HTMLElement|null} */
    _previousFocus: null,

    /**
     * Initialize modal system
     */
    init() {
        this.overlay = document.getElementById('input-modal');
        if (!this.overlay) {
            console.warn('Modal overlay not found');
            return;
        }

        this.icon = document.getElementById('modal-icon');
        this.title = document.getElementById('modal-title');
        this.label = document.getElementById('modal-label');
        this.input = /** @type {HTMLInputElement} */ (document.getElementById('modal-input'));
        this.cancelBtn = /** @type {HTMLButtonElement} */ (document.getElementById('modal-cancel-btn'));
        this.confirmBtn = /** @type {HTMLButtonElement} */ (document.getElementById('modal-confirm-btn'));
        this.closeBtn = /** @type {HTMLButtonElement} */ (document.getElementById('modal-close-btn'));

        // Event listeners
        this.errorEl = document.getElementById('modal-error');

        this.cancelBtn?.addEventListener('click', () => this.close(false));
        this.closeBtn?.addEventListener('click', () => this.close(false));
        this.confirmBtn?.addEventListener('click', () => this.confirm());

        // Close on overlay click
        this.overlay.addEventListener('click', (e) => {
            if (e.target === this.overlay) {
                this.close(false);
            }
        });

        // Trap Tab focus within the dialog while it is open.
        this.overlay.addEventListener('keydown', (e) => {
            if (e.key === 'Tab') this._trapFocus(e);
        });

        // Clear inline error as soon as the user starts typing
        this.input?.addEventListener('input', () => this.clearError());

        // Handle Enter and Escape keys
        this.input?.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') {
                e.preventDefault();
                this.confirm();
            } else if (e.key === 'Escape') {
                this.close(false);
            }
        });

        // Escape in panel mode (input isn't focused so the above handler won't fire)
        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape' && this._panelMode && !this.overlay?.classList.contains('hidden')) {
                this.close(false);
            }
        });
    },

    /** @param {string} message */
    showError(message) {
        this.input?.classList.add('modal-input--error');
        if (this.errorEl) {
            this.errorEl.textContent = message;
            this.errorEl.classList.remove('hidden');
        }
    },

    clearError() {
        this.input?.classList.remove('modal-input--error');
        if (this.errorEl) {
            this.errorEl.textContent = '';
            this.errorEl.classList.add('hidden');
        }
    },

    /**
     * Show input modal (replacement for prompt())
     * @param {Object} options - Modal configuration
     * @param {string} [options.title] - Modal title
     * @param {string} [options.label] - Input label
     * @param {string} [options.placeholder] - Input placeholder
     * @param {string} [options.value] - Initial input value
     * @param {string} [options.icon] - Font Awesome icon class (e.g., 'fa-folder-plus')
     * @param {string} [options.confirmText] - Confirm button text
     * @param {string} [options.cancelText] - Cancel button text
     * @param {(value: string) => Promise<void>} [options.action] - Async action called on confirm.
     *   Throw an Error to keep the modal open and display the error message inline.
     *   When omitted the modal resolves immediately with the input value (legacy behaviour).
     * @returns {Promise<string|null>} - Resolves with input value or null if cancelled
     */
    prompt(options = {}) {
        return new Promise((resolve) => {
            const {
                title = 'Input',
                label = '',
                placeholder = '',
                value = '',
                icon = 'fa-keyboard',
                confirmText = null,
                cancelText = null,
                action = null
            } = options;

            // Set modal content - update the icon
            const iconContainer = document.querySelector('.modal-icon');
            if (iconContainer) {
                // Replace contents with a fresh <i> that icons.js will convert
                iconContainer.innerHTML = `<i id="modal-icon" class="fas ${icon}"></i>`;
                this.icon = document.getElementById('modal-icon');
                // Let icons.js convert it to SVG
                if (replaceIconsInElement) {
                    replaceIconsInElement(iconContainer);
                    this.icon = document.getElementById('modal-icon');
                }
            }
            this.title.textContent = title;
            this.label.textContent = label;
            this.input.placeholder = placeholder;
            this.input.value = value;

            if (confirmText) {
                this.confirmBtn.textContent = confirmText;
            } else {
                this.confirmBtn.textContent = i18n.t('actions.confirm');
            }

            if (cancelText) {
                this.cancelBtn.textContent = cancelText;
            } else {
                this.cancelBtn.textContent = i18n.t('actions.cancel');
            }

            this._action = action;
            this.clearError();

            // Set callbacks
            this.onConfirm = () => {
                const inputValue = this.input.value.trim();
                resolve(inputValue || null);
            };
            this.onCancel = () => resolve(null);

            // Show modal
            this.open();
        });
    },

    /**
     * Show modal for creating new folder
     * @param {(value: string) => Promise<void>} [action]
     * @returns {Promise<string|null>}
     */
    promptNewFolder(action = null) {
        return this.prompt({
            title: i18n.t('dialogs.new_folder_title'),
            label: i18n.t('dialogs.folder_name'),
            placeholder: i18n.t('dialogs.folder_placeholder'),
            icon: 'fa-folder-plus',
            confirmText: i18n.t('actions.create'),
            action
        });
    },

    /**
     * Show modal for renaming
     * @param {string} currentName - Current name of file/folder
     * @param {boolean} isFolder - Whether it's a folder
     * @param {(value: string) => Promise<void>} [action]
     * @returns {Promise<string|null>}
     */
    promptRename(currentName, isFolder = false, action = null) {
        this._selectNameOnly = !isFolder;

        return this.prompt({
            title: i18n.t('dialogs.rename_title'),
            label: i18n.t('dialogs.new_name'),
            placeholder: '',
            value: currentName,
            icon: isFolder ? 'fa-folder' : 'fa-file',
            confirmText: i18n.t('actions.rename'),
            action
        });
    },

    /**
     * Keep Tab focus cycling within the dialog container (focus trap).
     * @private
     * @param {KeyboardEvent} e
     */
    _trapFocus(e) {
        const container = this.overlay?.querySelector('.modal-container');
        if (!container) return;
        const focusables = /** @type {NodeListOf<HTMLElement>} */ (
            container.querySelectorAll(
                'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'
            )
        );
        if (!focusables.length) return;
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        if (e.shiftKey && document.activeElement === first) {
            e.preventDefault();
            last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
            e.preventDefault();
            first.focus();
        }
    },

    /**
     * Open the modal
     */
    open() {
        if (!this.overlay) return;

        this._previousFocus = /** @type {HTMLElement|null} */ (document.activeElement);
        this.confirmBtn.disabled = false;

        // Show overlay
        this.overlay.classList.remove('hidden');

        // Trigger animation
        requestAnimationFrame(() => {
            this.overlay.classList.add('active');
        });

        // Focus input after animation
        setTimeout(() => {
            this.input.focus();

            if (this._selectNameOnly) {
                // Select only the filename, excluding the extension
                const value = this.input.value;
                const lastDot = value.lastIndexOf('.');
                if (lastDot > 0) {
                    this.input.setSelectionRange(0, lastDot);
                } else {
                    this.input.select();
                }
                this._selectNameOnly = false;
            } else {
                this.input.select();
            }
        }, 100);
    },

    /**
     * Close the modal
     * @param {boolean} confirmed - Whether the action was confirmed
     */
    close(confirmed = false) {
        if (!this.overlay) return;

        this.clearError();
        this._action = null;
        this.overlay.classList.remove('active');

        const wasPanel = this._panelMode;

        setTimeout(() => {
            this.overlay.classList.add('hidden');

            if (!confirmed && this.onCancel) {
                this.onCancel();
            }

            // Clear callbacks
            this.onConfirm = null;
            this.onCancel = null;

            // Restore original modal-body content after a panel closes
            if (wasPanel) {
                const bodyEl = this.overlay?.querySelector('.modal-body');
                if (bodyEl) bodyEl.innerHTML = this._savedBodyHTML;
                this.overlay?.querySelector('.modal-container')?.classList.remove('modal-container--panel');
                this._panelMode = false;
                this._savedBodyHTML = '';
            }

            // Return focus to whatever was focused before the modal opened.
            this._previousFocus?.focus?.();
            this._previousFocus = null;
        }, 200);
    },

    /**
     * Confirm the action. When an async action is set, the modal stays open
     * until it resolves — closing only on success, showing the error inline on failure.
     */
    async confirm() {
        // Panel mode: delegate entirely to the caller-supplied onConfirm
        if (this._panelMode) {
            if (this.onConfirm) this.onConfirm();
            this.close(true);
            return;
        }

        if (!this._action) {
            if (this.onConfirm) this.onConfirm();
            this.close(true);
            return;
        }

        const inputValue = this.input.value.trim();
        if (!inputValue) return;

        this.clearError();
        this.confirmBtn.disabled = true;

        try {
            await this._action(inputValue);
            if (this.onConfirm) this.onConfirm();
            this.close(true);
        } catch (e) {
            this.showError(/** @type {Error} */ (e).message || 'An error occurred');
            this.confirmBtn.disabled = false;
            this.input.focus();
        }
    },

    /**
     * Open the modal with fully custom body content (panel mode).
     *
     * The caller supplies a pre-built HTMLElement as `content`; it is injected
     * into `.modal-body`, replacing the default label/input/error elements for
     * the lifetime of this panel.  The overlay, header, animation, footer
     * buttons, click-outside, and Escape handling all come from Modal.
     *
     * Original `.modal-body` innerHTML is restored automatically when the
     * panel closes.
     *
     * @param {Object} options
     * @param {string}        options.title
     * @param {string}        [options.icon]             - Font Awesome class, default 'fa-share-alt'
     * @param {HTMLElement}   options.content            - DOM node to inject into .modal-body
     * @param {string}        [options.confirmText]      - Confirm button label
     * @param {string}        [options.cancelText]       - Cancel button label
     * @param {boolean}       [options.confirmDisabled]  - Initial disabled state of the confirm button
     * @param {() => void}    [options.onConfirm]        - Called when Confirm is clicked
     * @param {() => void}    [options.onCancel]         - Called when Cancel / close is triggered
     */
    openPanel({ title, icon = 'fa-share-alt', content, confirmText = null, cancelText = null, confirmDisabled = false, onConfirm = null, onCancel = null }) {
        if (!this.overlay) return;

        this._panelMode = true;

        // ── Header ──────────────────────────────────────────────────────────
        const iconContainer = this.overlay.querySelector('.modal-icon');
        if (iconContainer) {
            iconContainer.innerHTML = `<i class="fas ${icon}"></i>`;
            if (replaceIconsInElement) replaceIconsInElement(iconContainer);
        }
        if (this.title) this.title.textContent = title;

        // ── Body swap ───────────────────────────────────────────────────────
        const bodyEl = this.overlay.querySelector('.modal-body');
        if (bodyEl) {
            this._savedBodyHTML = bodyEl.innerHTML;
            bodyEl.replaceChildren(content);
        }

        // ── Container size modifier ──────────────────────────────────────────
        this.overlay.querySelector('.modal-container')?.classList.add('modal-container--panel');

        // ── Footer buttons ──────────────────────────────────────────────────
        if (this.confirmBtn) {
            this.confirmBtn.textContent = confirmText ?? i18n.t('actions.apply', 'Apply');
            this.confirmBtn.disabled = confirmDisabled;
        }
        if (this.cancelBtn) {
            this.cancelBtn.textContent = cancelText ?? i18n.t('actions.cancel');
        }

        // ── Callbacks ───────────────────────────────────────────────────────
        this.onConfirm = onConfirm;
        this.onCancel = onCancel;
        this._action = null;
        this.clearError();

        // ── Show overlay (same animation as prompt, no input focus) ─────────
        this.overlay.classList.remove('hidden');
        requestAnimationFrame(() => {
            this.overlay.classList.add('active');
        });
    }
};

// Initialize when DOM is ready
document.addEventListener('DOMContentLoaded', () => {
    Modal.init();
});

// Export for use in other modules
export { Modal };
