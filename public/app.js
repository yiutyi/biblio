// Biblio - E-book Library Browser
class BiblioApp {
    constructor() {
        this.libraries = [];
        this.currentLibraryId = null;
        this.currentLibrary = null;
        this.allBooks = [];
        // Books currently loaded from the server for the current filter/sort
        // (never the whole library: pages are fetched on demand).
        this.filteredBooks = [];
        this.totalBooks = 0;
        this.authors = [];
        this.tags = [];
        this.series = [];
        this.selectedBook = null;
        this.selectedBookId = null;
        this.selectedBookLibraryId = null;
        this.selectedAuthors = new Set();
        this.selectedTags = new Set();
        this.selectedSeries = new Set();
        this.selectedFormats = new Set();
        this.sortMethod = 'recent';
        this.searchTerm = '';
        this.booksPerPage = 100;      // rows/covers appended per render
        this.pageSize = 300;          // books fetched per request
        this.displayedBooksCount = 0;
        this.isLoadingMore = false;
        // Scroll list is windowed: past this many cards the oldest are dropped
        // from the DOM and replaced by a spacer of the same height, so the list
        // can grow to the whole library without the document getting huge.
        this.maxDomItems = 1000;
        this.domTrimmed = 0;          // how many leading items are only a spacer
        this.isAuthenticated = false;
        this.currentUsername = null;

        // Splitter drag state
        this.resizing = null;

        // View mode and table settings
        this.currentViewMode = 'grid';
        this.coverSize = 140;
        this.favorites = [];
        this.baseOffset = 0;
        this.pendingOffset = 0;
        this.tableColumnWidths = {
            title: 30,
            authors: 20,
            series: 15,
            publisher: 15,
            rating: 10,
            pubdate: 10
        };
        this.columnVisibility = {
            title: true,
            authors: true,
            series: true,
            publisher: true,
            rating: true,
            pubdate: true
        };

        // Track which library each view was rendered for
        this.gridRenderedForLibrary = null;
        this.tableRenderedForLibrary = null;
    }

    // Cookie Management Methods
    setCookie(name, value, days = 30) {
        const d = new Date();
        d.setTime(d.getTime() + (days * 24 * 60 * 60 * 1000));
        const expires = "expires=" + d.toUTCString();
        document.cookie = name + "=" + encodeURIComponent(JSON.stringify(value)) + ";" + expires + ";path=/";
    }

    getCookie(name) {
        const nameEQ = name + "=";
        const cookies = document.cookie.split(';');
        for (let cookie of cookies) {
            cookie = cookie.trim();
            if (cookie.indexOf(nameEQ) === 0) {
                try {
                    return JSON.parse(decodeURIComponent(cookie.substring(nameEQ.length)));
                } catch (e) {
                    return null;
                }
            }
        }
        return null;
    }

    saveAppState() {
        const state = {
            currentLibraryId: this.currentLibraryId,
            selectedAuthors: Array.from(this.selectedAuthors),
            selectedTags: Array.from(this.selectedTags),
            selectedSeries: Array.from(this.selectedSeries),
            selectedFormats: Array.from(this.selectedFormats),
            searchTerm: this.searchTerm,
            sortMethod: this.sortMethod,
            displayedBooksCount: this.displayedBooksCount,
            selectedBookId: this.selectedBookId,
            selectedBookLibraryId: this.selectedBookLibraryId
        };
        this.setCookie('biblioAppState', state);
    }

    loadAppState() {
        const state = this.getCookie('biblioAppState');
        if (state) {
            this.currentLibraryId = state.currentLibraryId;
            this.selectedAuthors = new Set((state.selectedAuthors || []).map(id => parseInt(id)));
            this.selectedTags = new Set((state.selectedTags || []).map(id => parseInt(id)));
            this.selectedSeries = new Set((state.selectedSeries || []).map(id => parseInt(id)));
            this.selectedFormats = new Set(state.selectedFormats || []);
            this.searchTerm = state.searchTerm || '';
            this.sortMethod = state.sortMethod || 'recent';
            this.displayedBooksCount = state.displayedBooksCount || 0;
            this.selectedBookId = state.selectedBookId || null;
            this.selectedBookLibraryId = state.selectedBookLibraryId || null;
            return state;
        }
        return null;
    }

    restoreFilterUI() {
        // Restore search input
        const searchInput = document.getElementById('searchInput');
        if (searchInput) {
            searchInput.value = this.searchTerm;
        }

        // Restore author checkboxes
        this.selectedAuthors.forEach(authorId => {
            const checkbox = document.getElementById(`author_${authorId}`);
            if (checkbox) {
                checkbox.checked = true;
            }
        });

        // Restore tag checkboxes
        this.selectedTags.forEach(tagId => {
            const checkbox = document.getElementById(`tag_${tagId}`);
            if (checkbox) {
                checkbox.checked = true;
            }
        });

        // Restore series checkboxes
        this.selectedSeries.forEach(seriesId => {
            const checkbox = document.getElementById(`series_${seriesId}`);
            if (checkbox) {
                checkbox.checked = true;
            }
        });

        // Restore format checkboxes
        this.selectedFormats.forEach(format => {
            const checkbox = document.getElementById(`format_${format}`);
            if (checkbox) {
                checkbox.checked = true;
            }
        });

        // Update count displays
        this.updateAuthorCount();
        this.updateFormatCount();
        this.updateTagCount();
        this.updateSeriesCount();
        this.renderActiveChips();
    }

    async init() {
        console.log('Initializing Biblio App...');

        // Load view preferences
        this.loadViewPreferences();

        // No login gate on purpose: this is a local single-user library browser and
        // the API is open on 127.0.0.1, so go straight into the app.
        const authState = this.loadAuthState();
        this.isAuthenticated = true;
        this.currentUsername = (authState && authState.username) || 'local';
        if (!authState) this.saveAuthState(this.currentUsername, 'admin');

        this.showMainApp();
        this.checkAdminStatus();
        this.loadFavorites();
        this.renderFavorites();

        const savedState = this.loadAppState();

        await this.loadLibraries();
        this.setupEventListeners();

        // Restore filter section collapsed/expanded states
        this.restoreFilterSectionStates();

        // If there was a saved library, select it; otherwise select first library
        if (savedState && savedState.currentLibraryId && this.libraries.some(lib => lib.id === savedState.currentLibraryId)) {
            await this.selectLibrary(savedState.currentLibraryId);
        } else if (this.libraries.length > 0) {
            await this.selectLibrary(this.libraries[0].id);
        }

        // After library is loaded, restore the selected book if it exists
        if (this.selectedBookId && this.selectedBookLibraryId) {
            await this.restoreSelectedBook();
        }

        // Update view display with saved preferences
        this.updateViewDisplay();
    }

    checkAdminStatus() {
        // Check if user has admin role from localStorage
        const auth = this.loadAuthState();
        const adminBtn = document.getElementById('adminBtn');
        if (!adminBtn) return;

        if (auth && auth.role === 'admin') {
            adminBtn.style.display = 'inline-block';
        } else {
            adminBtn.style.display = 'none';
        }
    }

    // Authentication Methods
    saveAuthState(username, role = 'reader') {
        const authState = {
            isAuthenticated: true,
            username: username,
            role: role,
            timestamp: Date.now()
        };
        localStorage.setItem('biblio_auth', JSON.stringify(authState));
    }

    loadAuthState() {
        try {
            const authState = localStorage.getItem('biblio_auth');
            return authState ? JSON.parse(authState) : null;
        } catch (error) {
            console.error('Error loading auth state:', error);
            this.clearAuthState();
            return null;
        }
    }

    clearAuthState() {
        localStorage.removeItem('biblio_auth');
    }

    showLoginPage() {
        try {
            const contentArea = document.querySelector('.content-area');
            const appBar = document.querySelector('.app-bar');
            const bottomPanel = document.querySelector('.bottom-panel');

            if (contentArea) contentArea.style.display = 'none';
            if (bottomPanel) bottomPanel.style.display = 'none';
            if (appBar) appBar.style.display = 'none';

            const existing = document.getElementById('loginContainer');
            if (existing) existing.remove();

            const loginContainer = document.createElement('div');
            loginContainer.id = 'loginContainer';
            loginContainer.innerHTML = `
                <div class="login-card">
                    <div class="login-head">
                        <div class="brand-mark" aria-hidden="true">
                            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M4 5.4C4 4.6 4.6 4 5.4 4H10.8V20H5.4C4.6 20 4 19.4 4 18.6V5.4Z" />
                                <path d="M13.2 4H18.6C19.4 4 20 4.6 20 5.4V18.6C20 19.4 19.4 20 18.6 20H13.2V4Z" />
                            </svg>
                        </div>
                        <div class="login-title">Biblio</div>
                        <div class="login-sub">E-book library browser</div>
                    </div>

                    <form id="loginForm" autocomplete="off">
                        <div class="login-field">
                            <label for="usernameInput">Username</label>
                            <input type="text" id="usernameInput" placeholder="Enter username"
                                autocomplete="username" spellcheck="false">
                        </div>
                        <div class="login-field">
                            <label for="passwordInput">Password</label>
                            <input type="password" id="passwordInput" placeholder="Enter password"
                                autocomplete="current-password">
                        </div>
                        <button class="login-submit" type="submit">Sign in</button>
                    </form>

                    <div id="loginError"></div>
                    <div class="login-foot">Private library · served locally</div>
                </div>
            `;

            document.body.appendChild(loginContainer);

            const passwordField = document.getElementById('passwordInput');
            const usernameField = document.getElementById('usernameInput');

            // Keep browsers from restoring a stale password into the field.
            if (passwordField) {
                passwordField.setAttribute('readonly', '');
                passwordField.value = '';
                setTimeout(() => {
                    if (passwordField) {
                        passwordField.removeAttribute('readonly');
                        passwordField.value = '';
                    }
                }, 50);
            }
            if (usernameField) usernameField.value = '';
            if (usernameField) usernameField.focus();

            const loginForm = document.getElementById('loginForm');
            if (loginForm) {
                loginForm.addEventListener('submit', (e) => this.handleLogin(e));
            }
        } catch (error) {
            console.error('Error showing login page:', error);
        }
    }

    async handleLogin(e) {
        e.preventDefault();

        const username = document.getElementById('usernameInput').value;
        const password = document.getElementById('passwordInput').value;
        const errorDiv = document.getElementById('loginError');

        if (!username || !password) {
            errorDiv.textContent = 'Please enter username and password';
            errorDiv.style.display = 'block';
            return;
        }

        try {
            const response = await fetch('/api/auth/login', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({ username, password })
            });

            const data = await response.json();

            if (data.success) {
                this.isAuthenticated = true;
                this.currentUsername = username;
                // Extract role from login response, default to 'reader' if not provided
                const role = data.data && data.data.role ? data.data.role : 'reader';
                this.saveAuthState(username, role);

                // Remove login page
                const loginContainer = document.getElementById('loginContainer');
                loginContainer.remove();

                // Show main app
                this.showMainApp();

                // Check admin status AFTER auth state is saved
                this.checkAdminStatus();

                // Initialize app
                const savedState = this.loadAppState();
                await this.loadLibraries();
                this.setupEventListeners();

                if (savedState && savedState.currentLibraryId && this.libraries.some(lib => lib.id === savedState.currentLibraryId)) {
                    await this.selectLibrary(savedState.currentLibraryId);
                } else if (this.libraries.length > 0) {
                    await this.selectLibrary(this.libraries[0].id);
                }
            } else {
                errorDiv.textContent = data.error || 'Login failed';
                errorDiv.style.display = 'block';
                // Clear password field on failed login for security
                document.getElementById('passwordInput').value = '';
            }
        } catch (error) {
            console.error('Login error:', error);
            errorDiv.textContent = 'An error occurred during login';
            errorDiv.style.display = 'block';
        }
    }

    showMainApp() {
        const contentArea = document.querySelector('.content-area');
        const appBar = document.querySelector('.app-bar');
        const bottomPanel = document.querySelector('.bottom-panel');

        if (contentArea) contentArea.style.display = 'flex';
        if (appBar) appBar.style.display = 'flex';
        if (bottomPanel) bottomPanel.style.display = 'flex';

        const userInfo = document.getElementById('userInfo');
        if (!userInfo) {
            const userInfoDiv = document.createElement('div');
            userInfoDiv.id = 'userInfo';
            const name = this.escapeHtml(this.currentUsername || '');
            const initial = (this.currentUsername || '?').trim().charAt(0) || '?';
            userInfoDiv.innerHTML = `
                <span class="u-avatar">${this.escapeHtml(initial)}</span>
                <span class="u-name">${name}</span>
                <button class="icon-btn" id="logoutBtn" title="Sign out" aria-label="Sign out">
                    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                        stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M14 8V5.6c0-.9-.7-1.6-1.6-1.6H5.6C4.7 4 4 4.7 4 5.6v12.8c0 .9.7 1.6 1.6 1.6h6.8c.9 0 1.6-.7 1.6-1.6V16" />
                        <path d="M10 12h10M17 9l3 3-3 3" />
                    </svg>
                </button>
            `;
            appBar.appendChild(userInfoDiv);
            userInfoDiv.querySelector('#logoutBtn').addEventListener('click', () => this.handleLogout());
        }
    }

    async handleLogout() {
        try {
            await fetch('/api/auth/logout', {
                method: 'POST'
            });
        } catch (error) {
            console.error('Logout error:', error);
        }

        this.isAuthenticated = false;
        this.currentUsername = null;
        this.clearAuthState();

        // Hide admin button
        const adminBtn = document.getElementById('adminBtn');
        if (adminBtn) {
            adminBtn.style.display = 'none';
        }

        // Remove user info
        const userInfo = document.getElementById('userInfo');
        if (userInfo) userInfo.remove();

        // Hide main content and show login page
        document.querySelector('.content-area').style.display = 'none';
        this.showLoginPage();
    }

    async loadLibraries() {
        try {
            this.updateStatus('Loading libraries...');
            const response = await fetch('/api/libraries');
            const data = await response.json();

            if (data.success) {
                this.libraries = data.data || [];
                this.renderLibraries();
            } else {
                console.error('Failed to load libraries:', data.error);
                this.updateStatus('Failed to load libraries');
            }
        } catch (error) {
            console.error('Error loading libraries:', error);
            this.updateStatus('Error loading libraries');
        }
    }

    renderLibraries() {
        const librariesList = document.getElementById('librariesList');
        if (!librariesList) return;
        librariesList.innerHTML = '';

        const total = document.getElementById('libraryTotal');
        if (total) total.textContent = this.libraries.length ? this.formatNumber(this.libraries.length) : '';

        if (this.libraries.length === 0) {
            librariesList.innerHTML = '<div class="fsec-empty">No libraries found. Add Calibre libraries to the libraries folder.</div>';
            return;
        }

        this.libraries.forEach(lib => {
            const item = document.createElement('div');
            item.className = 'filter-item';
            const checked = this.currentLibraryId === lib.id ? 'checked' : '';
            item.innerHTML = `
                <input type="radio" name="library" id="lib_${lib.id}" value="${lib.id}" ${checked}
                       onchange="app.selectLibrary('${lib.id}')">
                <label for="lib_${lib.id}">
                    <span class="fx-name">${this.escapeHtml(lib.name)}</span>
                    <span class="fx-count">${this.formatNumber(lib.book_count || 0)}</span>
                </label>
            `;
            librariesList.appendChild(item);
        });
    }

    async selectLibrary(libraryId) {
        try {
            this.currentLibraryId = libraryId;
            this.selectedBook = null;
            this.displayedBooksCount = 0;

            // Check if we're switching to a different library
            const isSwitchingLibrary = this.currentLibrary && this.currentLibrary.id !== libraryId;

            // If switching libraries, clear all filters
            if (isSwitchingLibrary) {
                this.selectedAuthors.clear();
                this.selectedTags.clear();
                this.selectedSeries.clear();
                this.selectedFormats.clear();
                this.searchTerm = '';
                this.sortMethod = 'recent';
            }

            // Update radio button
            document.querySelectorAll('input[name="library"]').forEach(radio => {
                radio.checked = radio.value === libraryId;
            });

            const lib = this.libraries.find(l => l.id === libraryId);
            if (lib) {
                this.currentLibrary = lib;
                document.getElementById('statusLibrary').textContent = lib.name;
                const brandLibrary = document.getElementById('brandLibrary');
                if (brandLibrary) brandLibrary.textContent = lib.name;
                const totalEl = document.getElementById('statusBooks');
                if (totalEl) totalEl.textContent = this.formatNumber(lib.book_count || 0);
            }

            this.updateStatus('Loading books...');
            await this.loadBooks();
            await this.loadFilters();

            // If switching libraries, clear UI; otherwise restore saved state
            if (isSwitchingLibrary) {
                const searchInput = document.getElementById('searchInput');
                if (searchInput) {
                    searchInput.value = '';
                }

                const sortSelect = document.getElementById('sortSelect');
                if (sortSelect) {
                    sortSelect.value = 'recent';
                }
                // Update filter count badges to show 0
                this.updateAuthorCount();
                this.updateTagCount();
                this.updateSeriesCount();
                this.updateFormatCount();
                // Save the new library ID with cleared filters to cookie
                this.saveAppState();
            } else {
                // On initial load, restore saved state if it exists for this library
                const savedState = this.getCookie('biblioAppState');
                if (savedState && savedState.currentLibraryId === libraryId) {
                    this.selectedAuthors = new Set((savedState.selectedAuthors || []).map(id => parseInt(id)));
                    this.selectedTags = new Set((savedState.selectedTags || []).map(id => parseInt(id)));
                    this.selectedSeries = new Set((savedState.selectedSeries || []).map(id => parseInt(id)));
                    this.selectedFormats = new Set(savedState.selectedFormats || []);
                    this.searchTerm = savedState.searchTerm || '';
                    this.sortMethod = savedState.sortMethod || 'recent';
                    this.restoreFilterUI();
                    // loadBooks() above already fetched an unfiltered first page;
                    // only refetch when the restored state actually filters.
                    if (this.searchTerm || this.selectedAuthors.size || this.selectedTags.size ||
                        this.selectedSeries.size || this.selectedFormats.size ||
                        this.sortMethod !== 'recent') {
                        this.applyFilters();
                    }
                }
            }

            this.updateStatus('Ready');
        } catch (error) {
            console.error('Error selecting library:', error);
            this.updateStatus('Error loading library');
        }
    }

    // Build the query string for one page of the current filter + sort state.
    // The server does the filtering, so only the visible page is transferred.
    booksQuery(offset, limit) {
        const params = new URLSearchParams();
        params.set('offset', String(offset));
        params.set('limit', String(limit || this.pageSize));

        if (this.searchTerm) {
            params.set('search', this.searchTerm);
        }
        if (this.sortMethod && this.sortMethod !== 'recent') {
            params.set('sort', this.sortMethod);
        }
        const names = (ids, list) => Array.from(ids)
            .map(id => {
                const item = list.find(x => x.id === parseInt(id));
                return item ? item.name : null;
            })
            .filter(name => !!name);

        const authorNames = names(this.selectedAuthors, this.authors);
        if (authorNames.length) params.set('authors', authorNames.join('|'));

        const tagNames = names(this.selectedTags, this.tags);
        if (tagNames.length) params.set('tags', tagNames.join('|'));

        const seriesNames = names(this.selectedSeries, this.series);
        if (seriesNames.length) params.set('series', seriesNames.join('|'));

        if (this.selectedFormats.size) {
            params.set('formats', Array.from(this.selectedFormats).join('|'));
        }
        return params.toString();
    }

    async fetchBooksPage(offset) {
        // `baseOffset` lets the random-shelf jump land somewhere in the middle
        // of the library while keeping infinite scroll arithmetic simple.
        const absolute = (this.baseOffset || 0) + offset;
        const url = `/api/libraries/${this.currentLibraryId}/books?${this.booksQuery(absolute)}`;
        const response = await fetch(url);
        return await response.json();
    }

    // "Surprise me": jump to a random page of the current filter/sort.
    async shuffleBooks() {
        if (!this.currentLibraryId) return;
        try {
            this.updateStatus('Picking a random shelf…');
            const response = await fetch(`/api/libraries/${this.currentLibraryId}/books?${this.booksQuery(0, 1)}`);
            const probe = await response.json();
            const total = probe.total || 0;
            if (!total) return;
            const span = Math.max(1, total - this.pageSize);
            const offset = Math.floor(Math.random() * span);
            this.pendingOffset = offset;
            this.loadBooks();
            this.updateStatus(`Random shelf · from #${this.formatNumber(offset + 1)}`);
        } catch (error) {
            console.error('Error shuffling books:', error);
            this.updateStatus('Could not pick a random shelf');
        }
    }

    async loadBooks() {
        try {
            this.baseOffset = this.pendingOffset || 0;
            this.pendingOffset = 0;
            // a fresh shelf always opens with the full chrome visible
            this._chromeScrollTop = 0;
            this.setChromeCollapsed(false);
            this.filteredBooks = [];
            this.displayedBooksCount = 0;
            this.totalBooks = 0;
            this.domTrimmed = 0;

            const booksGrid = document.getElementById('booksGrid');
            if (booksGrid) booksGrid.innerHTML = '';
            const tbody = document.getElementById('booksTableBody');
            if (tbody) tbody.innerHTML = '';

            const data = await this.fetchBooksPage(0);
            if (data.success) {
                this.filteredBooks = data.data || [];
                this.totalBooks = data.total || 0;
                const isFiltered = !!(this.searchTerm || this.selectedAuthors.size ||
                    this.selectedTags.size || this.selectedSeries.size || this.selectedFormats.size);
                const filteredEl = document.getElementById('statusFiltered');
                if (filteredEl) filteredEl.textContent = this.formatNumber(this.totalBooks);
                const matchesItem = document.getElementById('matchesItem');
                if (matchesItem) {
                    matchesItem.style.display = isFiltered ? 'flex' : 'none';
                }
                this.renderActiveChips();
                this.renderBooks();
            } else {
                console.error('Failed to load books:', data.error);
            }
        } catch (error) {
            console.error('Error loading books:', error);
        }
    }

    // Fetch the next page and append it. Used by infinite scroll and by the
    // "fill the screen" helpers when the first page is shorter than the viewport.
    async loadMoreBooks() {
        if (this.isLoadingMore) return false;
        if (this.filteredBooks.length >= this.totalBooks) return false;

        this.isLoadingMore = true;
        try {
            const data = await this.fetchBooksPage(this.filteredBooks.length);
            if (data.success && (data.data || []).length > 0) {
                this.filteredBooks = this.filteredBooks.concat(data.data);
                this.totalBooks = data.total || this.totalBooks;
                this.renderBooks();
                return true;
            }
        } catch (error) {
            console.error('Error loading more books:', error);
        } finally {
            this.isLoadingMore = false;
        }
        return false;
    }

    async loadFilters() {
        try {
            await Promise.all([
                this.loadAuthors(),
                this.loadTags(),
                this.loadSeries(),
                this.loadFormats()
            ]);
        } catch (error) {
            console.error('Error loading filters:', error);
        }
    }

    async loadAuthors() {
        try {
            const response = await fetch(`/api/libraries/${this.currentLibraryId}/authors`);
            const data = await response.json();

            if (data.success) {
                this.renderAuthors(data.data || []);
            }
        } catch (error) {
            console.error('Error loading authors:', error);
        }
    }

    renderAuthors(authors) {
        this.authors = authors || [];
        this.renderFilterSection('authors');
    }

    renderAuthorsFlat(sortedAuthors) {
        const authorsList = document.getElementById('authorsList');
        sortedAuthors.forEach(author => {
            const item = document.createElement('div');
            item.className = 'filter-item';
            const displayName = this.formatAuthorName(author.name);
            const bookCount = author.book_count || 0;
            item.innerHTML = `
                <input type="checkbox" id="author_${author.id}" value="${author.id}" 
                       onchange="app.toggleAuthorFilter(${author.id})">
                <label for="author_${author.id}">${displayName} (${bookCount})</label>
            `;
            authorsList.appendChild(item);
        });
    }

    renderAuthorsGrouped(sortedAuthors) {
        const authorsList = document.getElementById('authorsList');

        // Group authors by first letter of sort field
        const groups = {};
        sortedAuthors.forEach(author => {
            const lastNameKey = this.getLastNameFirstLetter(author);
            if (!groups[lastNameKey]) {
                groups[lastNameKey] = [];
            }
            groups[lastNameKey].push(author);
        });

        // Sort group keys alphabetically
        const sortedKeys = Object.keys(groups).sort();

        // Create collapsible sections for each letter
        sortedKeys.forEach((letter, index) => {
            const groupContainer = document.createElement('div');
            groupContainer.className = 'author-group';

            // Group header (clickable to expand/collapse)
            const header = document.createElement('div');
            header.className = 'author-group-header';
            header.style.cssText = 'cursor: pointer; padding: 8px 10px; background: #ecf0f1; font-weight: 500; border-radius: 3px; margin-bottom: 5px; user-select: none; display: flex; align-items: center; justify-content: space-between;';

            const letterSpan = document.createElement('span');
            letterSpan.textContent = `${letter} (${groups[letter].length})`;
            header.appendChild(letterSpan);

            const toggleIcon = document.createElement('span');
            toggleIcon.textContent = '▼';
            toggleIcon.style.cssText = 'font-size: 12px; transition: transform 0.2s;';
            header.appendChild(toggleIcon);

            // Group content (authors)
            const content = document.createElement('div');
            content.className = 'author-group-content';
            content.style.cssText = 'padding-left: 10px; display: none;';

            groups[letter].forEach(author => {
                const item = document.createElement('div');
                item.className = 'filter-item';
                const displayName = this.formatAuthorName(author.name);
                const bookCount = author.book_count || 0;
                item.innerHTML = `
                    <input type="checkbox" id="author_${author.id}" value="${author.id}" 
                           onchange="app.toggleAuthorFilter(${author.id})">
                    <label for="author_${author.id}">${displayName} (${bookCount})</label>
                `;
                content.appendChild(item);
            });

            // Toggle functionality
            header.addEventListener('click', () => {
                const isOpen = content.style.display !== 'none';
                content.style.display = isOpen ? 'none' : 'block';
                toggleIcon.style.transform = isOpen ? 'rotate(0deg)' : 'rotate(-180deg)';
            });

            // Expand first group by default
            if (index === 0) {
                content.style.display = 'block';
                toggleIcon.style.transform = 'rotate(-180deg)';
            }

            groupContainer.appendChild(header);
            groupContainer.appendChild(content);
            authorsList.appendChild(groupContainer);
        });
    }

    getLastNameFirstLetter(author) {
        // Use the sort field for consistent grouping
        const sortField = author.sort || author.name;

        // Get first letter and convert to uppercase
        const firstLetter = sortField.charAt(0).toUpperCase();
        // Return letter or # for non-alphabetic
        return /[A-Z]/.test(firstLetter) ? firstLetter : '#';
    }

    getFirstLetterFromString(str) {
        // Get first letter from a string and convert to uppercase
        const firstLetter = str.charAt(0).toUpperCase();
        // Return letter or # for non-alphabetic
        return /[A-Z]/.test(firstLetter) ? firstLetter : '#';
    }

    formatAuthorName(name) {
        // If name contains "|", reverse the parts (lastname|firstname -> firstname lastname)
        if (name.includes('|')) {
            const parts = name.split('|');
            if (parts.length === 2) {
                const lastname = parts[0].trim();
                const firstname = parts[1].trim();
                return firstname && lastname ? `${firstname} ${lastname}` : name;
            }
        }
        return name;
    }

    async loadTags() {
        try {
            const response = await fetch(`/api/libraries/${this.currentLibraryId}/tags`);
            const data = await response.json();

            if (data.success) {
                this.renderTags(data.data || []);
            }
        } catch (error) {
            console.error('Error loading tags:', error);
        }
    }

    renderTags(tags) {
        this.tags = tags || [];
        this.renderFilterSection('tags');
    }

    renderTagsFlat(sortedTags) {
        const tagsList = document.getElementById('tagsList');
        sortedTags.forEach(tag => {
            const item = document.createElement('div');
            item.className = 'filter-item';
            const bookCount = tag.book_count || 0;
            item.innerHTML = `
                <input type="checkbox" id="tag_${tag.id}" value="${tag.id}" 
                       onchange="app.toggleTagFilter(${tag.id})">
                <label for="tag_${tag.id}">${tag.name} (${bookCount})</label>
            `;
            tagsList.appendChild(item);
        });
    }

    renderTagsGrouped(sortedTags) {
        const tagsList = document.getElementById('tagsList');

        // Group tags by first letter
        const groups = {};
        sortedTags.forEach(tag => {
            const firstLetter = this.getFirstLetterFromString(tag.name);
            if (!groups[firstLetter]) {
                groups[firstLetter] = [];
            }
            groups[firstLetter].push(tag);
        });

        // Sort group keys alphabetically
        const sortedKeys = Object.keys(groups).sort();

        // Create collapsible sections for each letter
        sortedKeys.forEach((letter, index) => {
            const groupContainer = document.createElement('div');
            groupContainer.className = 'filter-group';

            // Group header (clickable to expand/collapse)
            const header = document.createElement('div');
            header.className = 'filter-group-header';
            header.style.cssText = 'cursor: pointer; padding: 8px 10px; background: #ecf0f1; font-weight: 500; border-radius: 3px; margin-bottom: 5px; user-select: none; display: flex; align-items: center; justify-content: space-between;';

            const letterSpan = document.createElement('span');
            letterSpan.textContent = `${letter} (${groups[letter].length})`;
            header.appendChild(letterSpan);

            const toggleIcon = document.createElement('span');
            toggleIcon.textContent = '▼';
            toggleIcon.style.cssText = 'font-size: 12px; transition: transform 0.2s;';
            header.appendChild(toggleIcon);

            // Group content (tags)
            const content = document.createElement('div');
            content.className = 'filter-group-content';
            content.style.cssText = 'padding-left: 10px; display: none;';

            groups[letter].forEach(tag => {
                const item = document.createElement('div');
                item.className = 'filter-item';
                const bookCount = tag.book_count || 0;
                item.innerHTML = `
                    <input type="checkbox" id="tag_${tag.id}" value="${tag.id}" 
                           onchange="app.toggleTagFilter(${tag.id})">
                    <label for="tag_${tag.id}">${tag.name} (${bookCount})</label>
                `;
                content.appendChild(item);
            });

            // Toggle functionality
            header.addEventListener('click', () => {
                const isOpen = content.style.display !== 'none';
                content.style.display = isOpen ? 'none' : 'block';
                toggleIcon.style.transform = isOpen ? 'rotate(0deg)' : 'rotate(-180deg)';
            });

            // Expand first group by default
            if (index === 0) {
                content.style.display = 'block';
                toggleIcon.style.transform = 'rotate(-180deg)';
            }

            groupContainer.appendChild(header);
            groupContainer.appendChild(content);
            tagsList.appendChild(groupContainer);
        });
    }

    async loadSeries() {
        try {
            const response = await fetch(`/api/libraries/${this.currentLibraryId}/series`);
            const data = await response.json();

            if (data.success) {
                this.renderSeries(data.data || []);
            }
        } catch (error) {
            console.error('Error loading series:', error);
        }
    }

    renderSeries(series) {
        this.series = series || [];
        this.renderFilterSection('series');
    }

    renderSeriesFlat(sortedSeries) {
        const seriesList = document.getElementById('seriesList');
        sortedSeries.forEach(s => {
            const item = document.createElement('div');
            item.className = 'filter-item';
            const bookCount = s.book_count || 0;
            item.innerHTML = `
                <input type="checkbox" id="series_${s.id}" value="${s.id}" 
                       onchange="app.toggleSeriesFilter(${s.id})">
                <label for="series_${s.id}">${s.name} (${bookCount})</label>
            `;
            seriesList.appendChild(item);
        });
    }

    renderSeriesGrouped(sortedSeries) {
        const seriesList = document.getElementById('seriesList');

        // Group series by first letter of sort field
        const groups = {};
        sortedSeries.forEach(s => {
            const sortField = s.sort || s.name;
            const firstLetter = this.getFirstLetterFromString(sortField);
            if (!groups[firstLetter]) {
                groups[firstLetter] = [];
            }
            groups[firstLetter].push(s);
        });

        // Sort group keys alphabetically
        const sortedKeys = Object.keys(groups).sort();

        // Create collapsible sections for each letter
        sortedKeys.forEach((letter, index) => {
            const groupContainer = document.createElement('div');
            groupContainer.className = 'filter-group';

            // Group header (clickable to expand/collapse)
            const header = document.createElement('div');
            header.className = 'filter-group-header';
            header.style.cssText = 'cursor: pointer; padding: 8px 10px; background: #ecf0f1; font-weight: 500; border-radius: 3px; margin-bottom: 5px; user-select: none; display: flex; align-items: center; justify-content: space-between;';

            const letterSpan = document.createElement('span');
            letterSpan.textContent = `${letter} (${groups[letter].length})`;
            header.appendChild(letterSpan);

            const toggleIcon = document.createElement('span');
            toggleIcon.textContent = '▼';
            toggleIcon.style.cssText = 'font-size: 12px; transition: transform 0.2s;';
            header.appendChild(toggleIcon);

            // Group content (series)
            const content = document.createElement('div');
            content.className = 'filter-group-content';
            content.style.cssText = 'padding-left: 10px; display: none;';

            groups[letter].forEach(s => {
                const item = document.createElement('div');
                item.className = 'filter-item';
                const bookCount = s.book_count || 0;
                item.innerHTML = `
                    <input type="checkbox" id="series_${s.id}" value="${s.id}" 
                           onchange="app.toggleSeriesFilter(${s.id})">
                    <label for="series_${s.id}">${s.name} (${bookCount})</label>
                `;
                content.appendChild(item);
            });

            // Toggle functionality
            header.addEventListener('click', () => {
                const isOpen = content.style.display !== 'none';
                content.style.display = isOpen ? 'none' : 'block';
                toggleIcon.style.transform = isOpen ? 'rotate(0deg)' : 'rotate(-180deg)';
            });

            // Expand first group by default
            if (index === 0) {
                content.style.display = 'block';
                toggleIcon.style.transform = 'rotate(-180deg)';
            }

            groupContainer.appendChild(header);
            groupContainer.appendChild(content);
            seriesList.appendChild(groupContainer);
        });
    }

    async loadFormats() {
        try {
            // The library is no longer loaded into the browser, so the server
            // reports how many books carry each format.
            const response = await fetch(`/api/libraries/${this.currentLibraryId}/formats`);
            const data = await response.json();

            this.formatsCount = {};
            this.formats = [];
            if (data.success) {
                (data.data || []).forEach(row => {
                    this.formatsCount[row.format] = row.count;
                    this.formats.push(row.format);
                });
            }

            this.renderFormats(this.formats, this.formatsCount);
        } catch (error) {
            console.error('Error loading formats:', error);
        }
    }

    renderFormats(formats, formatsCount) {
        this.formats = formats || [];
        this.formatsCount = formatsCount || {};
        this.renderFilterSection('formats');
    }

    toggleFormatFilter(format) {
        if (this.selectedFormats.has(format)) {
            this.selectedFormats.delete(format);
        } else {
            this.selectedFormats.add(format);
        }
        this.saveAppState();
        this.updateFormatCount();
        this.applyFilters();
    }

    toggleAuthorFilter(authorId) {
        authorId = parseInt(authorId);
        if (this.selectedAuthors.has(authorId)) {
            this.selectedAuthors.delete(authorId);
        } else {
            this.selectedAuthors.add(authorId);
        }
        this.saveAppState();
        this.updateAuthorCount();
        this.applyFilters();
    }

    toggleTagFilter(tagId) {
        tagId = parseInt(tagId);
        if (this.selectedTags.has(tagId)) {
            this.selectedTags.delete(tagId);
        } else {
            this.selectedTags.add(tagId);
        }
        this.saveAppState();
        this.updateTagCount();
        this.applyFilters();
    }

    toggleSeriesFilter(seriesId) {
        seriesId = parseInt(seriesId);
        if (this.selectedSeries.has(seriesId)) {
            this.selectedSeries.delete(seriesId);
        } else {
            this.selectedSeries.add(seriesId);
        }
        this.saveAppState();
        this.updateSeriesCount();
        this.applyFilters();
    }

    clearAuthors() {
        this.selectedAuthors.clear();
        this.saveAppState();
        this.renderAuthors(this.authors);
        this.updateAuthorCount();
        this.applyFilters();
    }

    clearFormats() {
        this.selectedFormats.clear();
        this.saveAppState();
        this.renderFormats(this.formats, this.formatsCount);
        this.updateFormatCount();
        this.applyFilters();
    }

    clearTags() {
        this.selectedTags.clear();
        this.saveAppState();
        this.renderTags(this.tags);
        this.updateTagCount();
        this.applyFilters();
    }

    clearSeries() {
        this.selectedSeries.clear();
        this.saveAppState();
        this.renderSeries(this.series);
        this.updateSeriesCount();
        this.applyFilters();
    }

    updateAuthorCount() {
        const span = document.getElementById('authorCount');
        if (span) {
            span.textContent = this.selectedAuthors.size > 0 ? String(this.selectedAuthors.size) : '';
        }
    }

    updateFormatCount() {
        const span = document.getElementById('formatCount');
        if (span) {
            span.textContent = this.selectedFormats.size > 0 ? String(this.selectedFormats.size) : '';
        }
    }

    updateTagCount() {
        const span = document.getElementById('tagCount');
        if (span) {
            span.textContent = this.selectedTags.size > 0 ? String(this.selectedTags.size) : '';
        }
    }

    updateSeriesCount() {
        const span = document.getElementById('seriesCount');
        if (span) {
            span.textContent = this.selectedSeries.size > 0 ? String(this.selectedSeries.size) : '';
        }
    }

    applyFilters() {
        // Filtering and sorting happen on the server: reset and fetch page 1.
        this.filterSeq = (this.filterSeq || 0) + 1;
        const seq = this.filterSeq;

        this.renderActiveChips();
        this.loadBooks().then(() => {
            if (seq !== this.filterSeq) return; // a newer request superseded this one
            if (this.currentViewMode === 'grid') {
                setTimeout(() => this.fillScreenWithCovers(), 0);
            } else if (this.currentViewMode === 'table') {
                setTimeout(() => this.fillScreenWithRows(), 0);
            }
        });
    }

    sortBooks(books) {
        const sorted = [...books];

        switch (this.sortMethod) {
            case 'title':
                sorted.sort((a, b) => {
                    const sortA = (a.sort || a.title).toLowerCase();
                    const sortB = (b.sort || b.title).toLowerCase();
                    return sortA.localeCompare(sortB);
                });
                break;
            case 'author':
                sorted.sort((a, b) => {
                    const authorA = a.authors[0] || '';
                    const authorB = b.authors[0] || '';
                    // Find the author objects to get their sort fields
                    const authorObjA = this.authors.find(auth => auth.name === authorA);
                    const authorObjB = this.authors.find(auth => auth.name === authorB);
                    const sortA = (authorObjA?.sort || authorA).toLowerCase();
                    const sortB = (authorObjB?.sort || authorB).toLowerCase();
                    return sortA.localeCompare(sortB);
                });
                break;
            case 'series':
                sorted.sort((a, b) => {
                    const seriesA = a.series || null;
                    const seriesB = b.series || null;
                    // Books without series go to the end
                    if (!seriesA && !seriesB) return 0;
                    if (!seriesA) return 1;
                    if (!seriesB) return -1;
                    const seriesCompare = seriesA.localeCompare(seriesB);
                    if (seriesCompare !== 0) {
                        return seriesCompare;
                    }
                    // If series names are the same, sort by series_index
                    const indexA = a.series_index ?? Infinity;
                    const indexB = b.series_index ?? Infinity;
                    return indexA - indexB;
                });
                break;
            case 'recent':
            default:
                // Keep original order (most recent first)
                break;
        }

        return sorted;
    }

    renderBooks() {
        // Check current view mode and render accordingly
        if (this.currentViewMode === 'table') {
            this.renderBooksTable();
        } else {
            this.renderBooksGrid();
        }
    }

    renderBooksGrid() {
        const booksGrid = document.getElementById('booksGrid');
        if (!booksGrid) return;

        // Clear grid on first render
        if (this.displayedBooksCount === 0) {
            booksGrid.innerHTML = '';
        }

        if (this.filteredBooks.length === 0) {
            booksGrid.innerHTML = `
                <div class="empty-state">
                    <svg width="34" height="34" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                        stroke-width="1.4" stroke-linecap="round">
                        <circle cx="10.5" cy="10.5" r="6.5" />
                        <path d="m15.4 15.4 4.2 4.2" />
                    </svg>
                    <strong>No books match</strong>
                    <span>Try a different search, or clear one of the filters.</span>
                </div>`;
            return;
        }

        // Calculate how many books to display
        const endIndex = Math.min(this.displayedBooksCount + this.booksPerPage, this.filteredBooks.length);
        const booksToDisplay = this.filteredBooks.slice(this.displayedBooksCount, endIndex);

        booksToDisplay.forEach(book => {
            const bookItem = document.createElement('div');
            bookItem.className = 'book-item' + (this.selectedBook && this.selectedBook.id === book.id ? ' selected' : '');
            bookItem.dataset.bookId = book.id;
            bookItem.onclick = () => this.selectBook(book);

            const coverDiv = document.createElement('div');
            coverDiv.className = 'book-cover';

            const img = document.createElement('img');
            // Native lazy loading: only covers near the viewport are
            // requested, so a long list does not fire hundreds of hits.
            img.loading = 'lazy';
            img.decoding = 'async';
            img.alt = book.title || 'Cover';
            const firstAuthor = (book.authors && book.authors.length > 0) ? book.authors[0] : 'Unknown';
            const coverUrl = `/api/libraries/${this.currentLibraryId}/books/${book.id}/cover`;
            const fallbackCover = () => {
                img.onerror = null;
                img.src = this.generateTemporaryCover(book.title, firstAuthor);
            };
            if (book.has_cover) {
                // One quiet retry before giving up. A mass first-paint batch
                // occasionally drops a request, and a permanent placeholder
                // would then contradict the details pane for the same book.
                img.onerror = () => {
                    if (!img.isConnected) return;
                    if (!img.dataset.retried) {
                        img.dataset.retried = '1';
                        setTimeout(() => {
                            if (!img.isConnected) return;
                            img.src = `${coverUrl}?retry=1`;
                        }, 700);
                    } else {
                        fallbackCover();
                    }
                };
                img.src = coverUrl;
            } else {
                img.src = this.generateTemporaryCover(book.title, firstAuthor);
            }
            coverDiv.appendChild(img);

            const favBtn = document.createElement('button');
            favBtn.type = 'button';
            favBtn.className = 'fav-btn' + (this.isFavorite(this.currentLibraryId, book.id) ? ' active' : '');
            favBtn.dataset.favKey = this.favKey(this.currentLibraryId, book.id);
            favBtn.title = 'Favorite';
            favBtn.setAttribute('aria-label', 'Toggle favorite');
            favBtn.innerHTML = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 20.4c-1-.7-8.5-5.4-10.2-9.6C.7 7.4 2.9 4 6.3 4c2 0 3.7 1 5.7 3 2-2 3.7-3 5.7-3 3.4 0 5.6 3.4 4.5 6.8-1.7 4.2-9.2 8.9-10.2 9.6Z" />
            </svg>`;
            favBtn.addEventListener('click', (e) => {
                e.stopPropagation();
                this.toggleFavorite({
                    lib: this.currentLibraryId,
                    id: book.id,
                    title: book.title,
                    authors: book.authors || []
                });
            });
            coverDiv.appendChild(favBtn);

            const titleDiv = document.createElement('div');
            titleDiv.className = 'book-title';
            titleDiv.textContent = book.title;

            const authorDiv = document.createElement('div');
            authorDiv.className = 'book-author';
            authorDiv.textContent = (book.authors && book.authors.length > 0)
                ? this.formatAuthorName(book.authors[0])
                : 'Unknown author';

            bookItem.appendChild(coverDiv);
            bookItem.appendChild(titleDiv);
            bookItem.appendChild(authorDiv);
            booksGrid.appendChild(bookItem);
        });

        this.displayedBooksCount = endIndex;
        this.gridRenderedForLibrary = this.currentLibraryId;

        // Setup infinite scroll on first render
        if (this.displayedBooksCount === this.booksPerPage) {
            this.setupInfiniteScroll();
        }

        this.trimGridDom();
    }

    // Keep the grid bounded: once the list gets long, drop the oldest cards and
    // stand a spacer of exactly the lost height in their place. Net height is
    // unchanged, so the scroll position and the visible content do not move.
    trimGridDom() {
        const grid = document.getElementById('booksGrid');
        if (!grid) return;
        const items = grid.querySelectorAll('.book-item');
        if (items.length <= this.maxDomItems) return;

        const removeCount = items.length - this.maxDomItems + 120;
        const scrollTopBefore = grid.scrollTop;
        const heightBefore = grid.scrollHeight;

        for (let i = 0; i < removeCount && i < items.length; i++) {
            items[i].remove();
        }

        const removed = heightBefore - grid.scrollHeight;
        if (removed <= 0) return;

        let spacer = grid.querySelector('.grid-spacer');
        if (!spacer) {
            spacer = document.createElement('div');
            spacer.className = 'grid-spacer';
            grid.insertBefore(spacer, grid.firstChild);
        }
        spacer.style.height = `${(parseFloat(spacer.style.height) || 0) + removed}px`;

        grid.scrollTop = scrollTopBefore;
        this.domTrimmed += removeCount;
    }

    // Scrolled back up into the spacer area: rebuild from the top so the books
    // that were dropped from the DOM become reachable again.
    resetGridToTop() {
        const grid = document.getElementById('booksGrid');
        if (!grid) return;
        this.domTrimmed = 0;
        this.displayedBooksCount = 0;
        grid.innerHTML = '';
        if (this.filteredBooks.length) {
            this.renderBooksGrid();
        }
        grid.scrollTop = 0;
    }

    setupInfiniteScroll() {
        const booksGrid = document.getElementById('booksGrid');

        // Remove existing listener if any
        booksGrid.removeEventListener('scroll', this.handleScroll);

        // Add new scroll listener
        this.handleScroll = () => {
            const scrollTop = booksGrid.scrollTop;
            this.trackChrome(scrollTop);
            if (this.isLoadingMore) {
                return;
            }
            // Nothing left to show at all
            if (this.displayedBooksCount >= this.filteredBooks.length &&
                this.filteredBooks.length >= this.totalBooks) {
                return;
            }

            const scrollHeight = booksGrid.scrollHeight;
            const clientHeight = booksGrid.clientHeight;

            // The viewport is inside the spacer that stands in for trimmed
            // The viewport is inside the spacer that stands in for trimmed
            // cards: there is nothing to look at, so rebuild the top of the list.
            const spacer = booksGrid.querySelector('.grid-spacer');
            const spacerHeight = spacer ? (parseFloat(spacer.style.height) || 0) : 0;
            if (spacerHeight > 0 && scrollTop + clientHeight < spacerHeight) {
                this.resetGridToTop();
                return;
            }

            // Load more books when user scrolls to 80% of the way down
            if (scrollTop + clientHeight >= scrollHeight * 0.8) {
                if (this.displayedBooksCount < this.filteredBooks.length) {
                    this.renderBooks();
                } else {
                    this.loadMoreBooks();
                }
            }
        };

        booksGrid.addEventListener('scroll', this.handleScroll);
    }

    // Reading posture: scrolling down a shelf hides the app bar, toolbar and
    // status bar so the grid gets every available pixel; any upward scroll
    // brings them straight back.
    trackChrome(scrollTop) {
        if (this._pinning) {
            this._chromeScrollTop = scrollTop;
            return;
        }
        const last = this._chromeScrollTop || 0;
        const delta = scrollTop - last;
        this._chromeScrollTop = scrollTop;
        if (delta > 4 && scrollTop > 120) {
            this.setChromeCollapsed(true);
        } else if (delta < -4 || scrollTop < 60) {
            this.setChromeCollapsed(false);
        }
    }

    setChromeCollapsed(collapsed) {
        if (this._chromeCollapsed === collapsed) return;
        this._chromeCollapsed = collapsed;
        document.body.classList.toggle('chrome-collapsed', collapsed);
    }

    async selectBook(book, libraryId) {
        this._selectSeq = (this._selectSeq || 0) + 1;
        this.selectedBook = book;
        this.selectedBookId = book.id;
        this.selectedBookLibraryId = libraryId || this.currentLibraryId;
        this.saveAppState();

        // Update selected styling for grid view
        document.querySelectorAll('.book-item').forEach(item => {
            item.classList.remove('selected');
        });

        // Update selected styling for table view
        document.querySelectorAll('.books-table tbody tr').forEach(row => {
            row.classList.remove('selected');
        });

        // Add selected class to appropriate element
        if (this.currentViewMode === 'grid') {
            const clicked = (typeof event !== 'undefined' && event && event.target)
                ? event.target.closest('.book-item')
                : null;
            const target = clicked || document.querySelector(`.book-item[data-book-id="${book.id}"]`);
            if (target) target.classList.add('selected');
        } else if (this.currentViewMode === 'table') {
            // For table view, find and highlight the row
            const selectedRow = document.querySelector(`tr[data-book-id="${book.id}"][data-library-id="${this.selectedBookLibraryId}"]`);
            if (selectedRow) {
                selectedRow.classList.add('selected');
            }
        }

        // Details slide in as a side peek whenever a book is picked;
        // the filter drawer gets out of the way.
        const panel = document.querySelector('.right-panel');
        const wasOpen = !!(panel && panel.classList.contains('open'));
        const tile = (this.currentViewMode === 'grid' && this.selectedBookId)
            ? document.querySelector(`.book-item[data-book-id="${book.id}"]`)
            : null;
        // measure before the width transition begins to move the grid
        const anchorTop = tile ? tile.getBoundingClientRect().top : null;
        document.querySelector('.left-panel')?.classList.remove('open');
        panel?.classList.add('open');

        this.renderBookDetails(book);
        if (tile && !wasOpen) this.pinClickedBook(book, tile, anchorTop);
    }

    // Opening the details pane steals 360px, so the shelf re-flows (9 -> 7
    // columns) and the card the reader just clicked can jump several rows.
    // Anchor it: on every frame of the width animation nudge the scroll back
    // so the card stays under the cursor, then make sure it is still on screen.
    pinClickedBook(book, item, anchorTop) {
        if (this.currentViewMode !== 'grid') return;
        const grid = document.getElementById('booksGrid');
        if (!grid || !item || anchorTop == null || !item.isConnected) return;

        const anchor = anchorTop;
        const started = performance.now();
        this._pinning = true;
        const step = () => {
            if (!item.isConnected) { this._pinning = false; return; }
            const dy = item.getBoundingClientRect().top - anchor;
            if (Math.abs(dy) > 0.5) grid.scrollTop += dy;
            if (performance.now() - started < 340) {
                requestAnimationFrame(step);
                return;
            }
            this._pinning = false;
            this._chromeScrollTop = grid.scrollTop;
            // nudge just enough to keep the whole card on screen, never jump a row
            const r = item.getBoundingClientRect();
            const gr = grid.getBoundingClientRect();
            if (r.top < gr.top + 8) {
                grid.scrollTop -= (gr.top + 8 - r.top);
            } else if (r.bottom > gr.bottom - 8) {
                grid.scrollTop += (r.bottom - gr.bottom + 8);
            }
        };
        requestAnimationFrame(step);
    }

    async restoreSelectedBook() {
        const seq = this._selectSeq || 0;
        try {
            // The library is paginated now, so the remembered book may not be in
            // the loaded page: ask the server for that one book instead.
            let book = this.filteredBooks.find(b => b.id === this.selectedBookId);
            if (!book && this.selectedBookId) {
                try {
                    const response = await fetch(`/api/libraries/${this.currentLibraryId}/books/${this.selectedBookId}`);
                    if (response.ok) {
                        const data = await response.json();
                        if (data.success && data.data) book = data.data;
                    }
                } catch (e) {
                    console.warn('Could not fetch remembered book:', e);
                }
            }

            if (seq !== (this._selectSeq || 0)) return; // reader already picked another book
            if (book) {
                // Book is in the current library, restore it
                this.selectedBook = book;
                this.renderBookDetails(book);

                // Highlight the book in the grid if visible
                document.querySelectorAll('.book-item').forEach(item => {
                    item.classList.remove('selected');
                });
                // Find and highlight the book item
                const bookItems = document.querySelectorAll('.book-item');
                for (let item of bookItems) {
                    if (item.querySelector('img')?.src?.includes(`/books/${this.selectedBookId}/`)) {
                        item.classList.add('selected');
                        break;
                    }
                }
            } else {
                // Show the first loaded book of the current library if there is one
                if (this.filteredBooks.length > 0) {
                    const firstBook = this.filteredBooks[0];
                    this.selectedBook = firstBook;
                    this.selectedBookId = firstBook.id;
                    this.selectedBookLibraryId = this.currentLibraryId;
                    this.saveAppState();
                    this.renderBookDetails(firstBook);

                    // Highlight the first book in the grid
                    document.querySelectorAll('.book-item').forEach(item => {
                        item.classList.remove('selected');
                    });
                    const bookItems = document.querySelectorAll('.book-item');
                    if (bookItems.length > 0) {
                        bookItems[0].classList.add('selected');
                    }
                } else {
                    // No books in current library, keep it clear
                    this.selectedBook = null;
                    this.selectedBookId = null;
                    this.selectedBookLibraryId = null;
                    this.saveAppState();
                    const noSelection = document.getElementById('noSelection');
                    const bookDetails = document.getElementById('bookDetails');
                    if (noSelection && bookDetails) {
                        noSelection.style.display = 'block';
                        bookDetails.classList.remove('active');
                    }
                }
            }
        } catch (error) {
            console.error('Error restoring selected book:', error);
        }
    }

    renderBookDetails(book) {
        const noSelection = document.getElementById('noSelection');
        const bookDetails = document.getElementById('bookDetails');

        if (noSelection) noSelection.style.display = 'none';
        if (bookDetails) bookDetails.classList.add('active');

        document.getElementById('detailTitle').textContent = book.title;

        // Authors as links: clicking one filters the library by that author.
        const authorsEl = document.getElementById('detailAuthors');
        if (authorsEl) {
            authorsEl.innerHTML = '';
            const names = (book.authors || []).slice(0, 6);
            if (!names.length) {
                authorsEl.textContent = 'Unknown author';
            }
            names.forEach((name, i) => {
                const authorId = this.findAuthorId(name);
                const node = document.createElement(authorId ? 'button' : 'span');
                if (authorId) {
                    node.type = 'button';
                    node.className = 'author-link';
                    node.addEventListener('click', () => this.toggleAuthorFilter(authorId));
                }
                node.textContent = this.formatAuthorName(name);
                authorsEl.appendChild(node);
                if (i < names.length - 1) {
                    const sep = document.createElement('span');
                    sep.textContent = '·';
                    sep.style.color = 'var(--muted)';
                    authorsEl.appendChild(sep);
                }
            });
        }

        // Series
        let seriesText = '';
        if (book.series && book.series_index) {
            seriesText = `${book.series} #${book.series_index}`;
        } else if (book.series) {
            seriesText = book.series;
        }
        document.getElementById('detailSeries').textContent = seriesText;
        this.toggleDetailSection('detailSeriesSection', seriesText !== '');

        // Tags as clickable chips
        const tagsEl = document.getElementById('detailTags');
        if (tagsEl) {
            tagsEl.innerHTML = '';
            const tags = (book.tags || []).slice(0, 40);
            tags.forEach(tag => {
                const tagId = this.findTagId(tag);
                const chip = document.createElement('button');
                chip.type = 'button';
                chip.className = 'tag-chip';
                chip.textContent = tag;
                if (tagId) chip.addEventListener('click', () => this.toggleTagFilter(tagId));
                tagsEl.appendChild(chip);
            });
            if (!tags.length) tagsEl.textContent = '—';
        }
        this.toggleDetailSection('detailTagsSection', (book.tags || []).length > 0);

        // Publisher
        document.getElementById('detailPublisher').textContent = book.publisher || '';
        this.toggleDetailSection('detailPublisherSection', !!book.publisher);

        // Published
        const pubdateText = book.pubdate ? new Date(book.pubdate).toLocaleDateString() : '';
        document.getElementById('detailPubdate').textContent = pubdateText;
        this.toggleDetailSection('detailPubdateSection', pubdateText !== '');

        // Rating
        const ratingEl = document.getElementById('detailRating');
        if (book.rating) {
            ratingEl.innerHTML = `${this.renderStars(book.rating)} <span style="font-size:11px;color:var(--muted)">${book.rating}/10</span>`;
        } else {
            ratingEl.textContent = '';
        }
        this.toggleDetailSection('detailRatingSection', !!book.rating);

        // Description (Calibre stores HTML here)
        const commentsEl = document.getElementById('detailComments');
        if (book.comments && book.comments.trim()) {
            commentsEl.innerHTML = book.comments;
            this.toggleDetailSection('detailCommentsSection', true);
        } else {
            commentsEl.innerHTML = '';
            this.toggleDetailSection('detailCommentsSection', false);
        }

        // Cover: show the grid's already-cached thumbnail immediately, then
        // upgrade to the full-size scan once it arrives. The old book's picture
        // must never linger, so every callback is fenced by a sequence number.
        const coverImage = document.getElementById('coverImage');
        const generatedCover = () => this.generateTemporaryCover(
            book.title,
            (book.authors || []).join(', ') || 'Unknown'
        );
        const base = `/api/libraries/${this.currentLibraryId}/books/${book.id}/cover`;
        const coverSeq = (this._coverSeq = (this._coverSeq || 0) + 1);
        coverImage.onerror = null;
        coverImage.onload = null;
        coverImage.classList.add('is-loading');
        coverImage.onload = () => {
            if (coverSeq !== this._coverSeq) return;
            coverImage.classList.remove('is-loading');
            // If the grid tile for this book had given up and painted a
            // generated plate, repair it now that the server answered.
            if (!coverImage.src.startsWith('data:')) {
                const tile = document.querySelector(`.book-item[data-book-id="${book.id}"] img`);
                if (tile && tile.src.startsWith('data:')) {
                    tile.dataset.retried = '1';
                    tile.src = coverImage.src.split('?')[0];
                }
            }
        };
        if (book.has_cover) {
            let retried = false;
            coverImage.onerror = () => {
                if (coverSeq !== this._coverSeq) return;
                if (!retried) {
                    retried = true;
                    coverImage.src = `${base}?retry=1`;
                    return;
                }
                coverImage.onerror = null;
                coverImage.src = generatedCover();
            };
            // thumbnail first: usually already decoded from the card itself
            coverImage.src = base;
            const full = new Image();
            full.decoding = 'async';
            full.onload = () => {
                if (coverSeq !== this._coverSeq || !full.naturalWidth) return;
                coverImage.classList.add('is-loading');
                coverImage.src = full.src;
            };
            full.onerror = () => { };
            full.src = `${base}?size=full`;
        } else {
            coverImage.src = generatedCover();
        }
        coverImage.style.display = 'block';

        // Re-scroll the panel to the top for a newly selected book
        const rightPanel = document.querySelector('.right-inner');
        if (rightPanel) rightPanel.scrollTop = 0;

        this.loadAndDisplayFormats(book.id);
        this.syncDetailFavButton();
    }

    toggleDetailSection(sectionId, show) {
        const section = document.getElementById(sectionId);
        if (section) {
            section.style.display = show ? 'flex' : 'none';
        }
    }

    toggleFilterSection(sectionId) {
        const content = document.getElementById(sectionId);
        const header = content ? content.previousElementSibling : null;
        const toggle = header ? header.querySelector('.filter-section-toggle') : null;

        if (content && toggle) {
            content.classList.toggle('collapsed');
            toggle.classList.toggle('collapsed');
            this.saveFilterSectionStates();
        }
    }

    saveFilterSectionStates() {
        const sections = ['librariesList', 'authorsList', 'seriesList', 'tagsList', 'formatsList'];
        const states = {};
        sections.forEach(sectionId => {
            const content = document.getElementById(sectionId);
            if (content) {
                states[sectionId] = content.classList.contains('collapsed');
            }
        });
        localStorage.setItem('biblioFilterSectionStates', JSON.stringify(states));
    }

    restoreFilterSectionStates() {
        const states = localStorage.getItem('biblioFilterSectionStates');
        if (states) {
            try {
                const stateObj = JSON.parse(states);
                Object.keys(stateObj).forEach(sectionId => {
                    const content = document.getElementById(sectionId);
                    const header = content ? content.previousElementSibling : null;
                    const toggle = header ? header.querySelector('.filter-section-toggle') : null;

                    if (content && toggle) {
                        if (stateObj[sectionId]) {
                            content.classList.add('collapsed');
                            toggle.classList.add('collapsed');
                        } else {
                            content.classList.remove('collapsed');
                            toggle.classList.remove('collapsed');
                        }
                    }
                });
            } catch (e) {
                console.error('Failed to restore filter section states:', e);
            }
        }
    }

    // A hand-set "paper plate" for books Calibre has no artwork for: warm paper,
    // a gold rule, serif title and a small author line. Deterministic per title
    // and cached, so a 300-book grid never redraws the same cover twice.
    // Faint ornamental tissue printed behind the title. Ten motifs, each with
    // randomised spacing/angle/scale, all derived from the title hash so the
    // same book always gets the same ornament while no two titles collide.
    paintCoverPattern(ctx, W, H, hash, plate) {
        const pat = document.createElement('canvas');
        pat.width = W;
        pat.height = H;
        const p = pat.getContext('2d');

        let seed = (hash >>> 0) || 7;
        const rnd = () => {
            seed = (seed * 1103515245 + 12345) >>> 0;
            return (seed >>> 8) / 16777216;
        };

        const style = hash % 10;
        p.strokeStyle = plate.rule;
        p.fillStyle = plate.rule;
        p.lineWidth = 1;
        p.globalAlpha = .55;

        if (style === 0) {
            // diagonal hatch with a random rake
            const step = 9 + Math.floor(rnd() * 9);
            p.save();
            p.translate(W / 2, H / 2);
            p.rotate(-0.32 - rnd() * 0.55);
            for (let i = -H * 1.4; i < H * 1.4; i += step) {
                p.beginPath();
                p.moveTo(-W, i);
                p.lineTo(W, i);
                p.stroke();
            }
            p.restore();
        } else if (style === 1) {
            // concentric rings growing out of one corner
            const cx = rnd() < 0.5 ? 0 : W;
            const cy = rnd() < 0.5 ? 0 : H;
            const step = 13 + Math.floor(rnd() * 10);
            for (let r = step; r < Math.hypot(W, H) * 1.15; r += step) {
                p.beginPath();
                p.arc(cx, cy, r, 0, Math.PI * 2);
                p.stroke();
            }
        } else if (style === 2) {
            // halftone dot screen, staggered rows
            const step = 13 + Math.floor(rnd() * 8);
            const off = step / 2;
            let row = 0;
            for (let y = 0; y < H + step; y += step, row++) {
                for (let x = row % 2 ? off : 0; x < W + step; x += step) {
                    const r = 1.1 + rnd() * 1.5;
                    p.beginPath();
                    p.arc(x, y, r, 0, Math.PI * 2);
                    p.fill();
                }
            }
        } else if (style === 3) {
            // scattered leaves with a vein
            const n = 26 + Math.floor(rnd() * 16);
            for (let i = 0; i < n; i++) {
                const x = rnd() * W;
                const y = H * 0.42 + rnd() * H * 0.62;
                const r = 7 + rnd() * 11;
                p.save();
                p.translate(x, y);
                p.rotate(rnd() * Math.PI);
                p.beginPath();
                p.ellipse(0, 0, r, r * 0.4, 0, 0, Math.PI * 2);
                p.stroke();
                p.beginPath();
                p.moveTo(-r, 0);
                p.lineTo(r, 0);
                p.stroke();
                p.restore();
            }
        } else if (style === 4) {
            // stacked sine waves
            const rows = 7 + Math.floor(rnd() * 5);
            const gap = H / (rows + 2);
            const amp = 5 + rnd() * 9;
            const freq = 0.011 + rnd() * 0.012;
            for (let r = 0; r < rows; r++) {
                const baseY = gap * (r + 2);
                p.beginPath();
                for (let x = 0; x <= W; x += 4) {
                    const y = baseY + Math.sin(x * freq + r * 1.3) * amp;
                    if (x === 0) p.moveTo(x, y); else p.lineTo(x, y);
                }
                p.stroke();
            }
        } else if (style === 5) {
            // art-deco fan rising from the base
            const cx = W / 2;
            const cy = H * 1.04;
            const rays = 20 + Math.floor(rnd() * 12);
            const len = H * (0.62 + rnd() * 0.28);
            for (let i = 0; i <= rays; i++) {
                const a = Math.PI - (Math.PI * i) / rays;
                p.beginPath();
                p.moveTo(cx, cy);
                p.lineTo(cx + Math.cos(a) * len, cy + Math.sin(a) * len * 0.92);
                p.stroke();
            }
        } else if (style === 6) {
            // loose overlapping circles
            const n = 9 + Math.floor(rnd() * 7);
            for (let i = 0; i < n; i++) {
                const x = -W * 0.3 + rnd() * W * 1.6;
                const y = H * 0.35 + rnd() * H * 0.75;
                const r = 40 + rnd() * 90;
                p.beginPath();
                p.arc(x, y, r, 0, Math.PI * 2);
                p.stroke();
            }
        } else if (style === 7) {
            // rotated diamond lattice
            const step = 22 + Math.floor(rnd() * 16);
            const size = 5 + rnd() * 6;
            p.save();
            p.translate(W / 2, H / 2);
            p.rotate(Math.PI / 4);
            for (let y = -H; y < H; y += step) {
                for (let x = -W; x < W; x += step) {
                    p.beginPath();
                    p.moveTo(x - size, y);
                    p.lineTo(x, y - size);
                    p.lineTo(x + size, y);
                    p.lineTo(x, y + size);
                    p.closePath();
                    p.stroke();
                }
            }
            p.restore();
        } else if (style === 8) {
            // pinstripes of alternating weight
            let x = 0;
            while (x < W) {
                const gap = 6 + rnd() * 16;
                p.lineWidth = rnd() < 0.25 ? 2 : 1;
                p.beginPath();
                p.moveTo(x, 0);
                p.lineTo(x, H);
                p.stroke();
                x += gap;
            }
            p.lineWidth = 1;
        } else {
            // scallop tiles
            const r = 16 + rnd() * 12;
            const stepX = r * 1.8;
            const stepY = r * 0.9;
            let row = 0;
            for (let y = H * 0.4; y < H + r; y += stepY, row++) {
                for (let x = row % 2 ? 0 : stepX / 2; x < W + stepX; x += stepX) {
                    p.beginPath();
                    p.arc(x, y, r, Math.PI, Math.PI * 2);
                    p.stroke();
                }
            }
        }

        // Mask the tissue away from the title block: a whisper at the top edge,
        // nothing behind the words, full strength over the lower third.
        const mask = p.createLinearGradient(0, 0, 0, H);
        mask.addColorStop(0, 'rgba(0,0,0,0.30)');
        mask.addColorStop(0.13, 'rgba(0,0,0,0)');
        mask.addColorStop(0.66, 'rgba(0,0,0,0)');
        mask.addColorStop(0.80, 'rgba(0,0,0,0.9)');
        mask.addColorStop(1, 'rgba(0,0,0,1)');
        p.globalCompositeOperation = 'destination-in';
        p.globalAlpha = 1;
        p.fillStyle = mask;
        p.fillRect(0, 0, W, H);

        const wasAlpha = ctx.globalAlpha;
        ctx.globalAlpha = .42;
        ctx.drawImage(pat, 0, 0);
        ctx.globalAlpha = wasAlpha;
    }

    generateTemporaryCover(title, author) {
        const cleanTitle = (title || 'Untitled').trim();
        const cleanAuthor = (author || '').trim();
        const cacheKey = cleanTitle + '|' + cleanAuthor;
        if (!this._coverCache) this._coverCache = new Map();
        const cached = this._coverCache.get(cacheKey);
        if (cached) return cached;

        const W = 360;
        const H = 540;
        const canvas = document.createElement('canvas');
        canvas.width = W;
        canvas.height = H;
        const ctx = canvas.getContext('2d');

        const plates = [
            { paper: '#F4EEE2', ink: '#2C2620', rule: '#B08B4F' },
            { paper: '#ECF0EA', ink: '#23302B', rule: '#59796B' },
            { paper: '#F5EAE5', ink: '#33262A', rule: '#AC6E5C' },
            { paper: '#E9EBF0', ink: '#262B36', rule: '#6B7891' },
            { paper: '#2C3034', ink: '#F2EDE3', rule: '#C7A76B' }
        ];
        let hash = 0;
        for (let i = 0; i < cleanTitle.length; i++) {
            hash = (hash * 31 + cleanTitle.charCodeAt(i)) >>> 0;
        }
        const plate = plates[hash % plates.length];
        const serif = '"Songti SC", "Noto Serif SC", "Noto Serif CJK SC", Georgia, serif';

        ctx.fillStyle = plate.paper;
        ctx.fillRect(0, 0, W, H);

        // soft print wash + spine shadow on the left edge
        const wash = ctx.createLinearGradient(0, 0, 0, H);
        wash.addColorStop(0, 'rgba(255,255,255,0.07)');
        wash.addColorStop(1, 'rgba(0,0,0,0.07)');
        ctx.fillStyle = wash;
        ctx.fillRect(0, 0, W, H);

        const spine = ctx.createLinearGradient(0, 0, 24, 0);
        spine.addColorStop(0, 'rgba(0,0,0,0.15)');
        spine.addColorStop(1, 'rgba(0,0,0,0)');
        ctx.fillStyle = spine;
        ctx.fillRect(0, 0, 24, H);

        // printed tissue, then the gold frame sits on top of it
        this.paintCoverPattern(ctx, W, H, hash, plate);

        ctx.globalAlpha = .45;
        ctx.strokeStyle = plate.rule;
        ctx.lineWidth = 1;
        ctx.strokeRect(22.5, 22.5, W - 45, H - 45);
        ctx.globalAlpha = 1;

        // Title: shrink to fit. CJK breaks per character, Latin breaks per word,
        // so an English title never gets chopped mid-word.
        const maxW = W - 96;
        const tokenize = text => {
            const tokens = [];
            let word = '';
            for (const ch of Array.from(text)) {
                if (/[A-Za-z0-9'’\-\u00C0-\u024F]/.test(ch)) {
                    word += ch;
                } else {
                    if (word) { tokens.push(word); word = ''; }
                    if (ch === '\n') tokens.push('\n');
                    else if (ch === ' ') tokens.push(' ');
                    else tokens.push(ch);
                }
            }
            if (word) tokens.push(word);
            return tokens;
        };
        const wrap = (text, font) => {
            ctx.font = font;
            const tokens = tokenize(text);
            const out = [];
            let line = '';
            for (const token of tokens) {
                if (token === '\n') {
                    out.push(line);
                    line = '';
                    continue;
                }
                if (token === ' ' && !line) continue;
                const test = line + token;
                if (ctx.measureText(test).width <= maxW || !line) {
                    // A single token wider than the column: split it by characters.
                    if (!line && ctx.measureText(test).width > maxW && token.length > 1) {
                        for (const ch of Array.from(token)) {
                            if (ctx.measureText(line + ch).width > maxW && line) {
                                out.push(line);
                                line = ch;
                            } else {
                                line += ch;
                            }
                        }
                    } else {
                        line = test;
                    }
                } else {
                    out.push(line.trimEnd());
                    line = token === ' ' ? '' : token;
                }
            }
            if (line.trim()) out.push(line.trimEnd());
            return out;
        };

        let size = 40;
        let lines = [];
        for (; size >= 18; size -= 1) {
            lines = wrap(cleanTitle, `600 ${size}px ${serif}`);
            if (lines.length <= 6 && lines.length * size * 1.4 <= 310) break;
        }
        ctx.font = `600 ${size}px ${serif}`;
        ctx.fillStyle = plate.ink;
        ctx.textAlign = 'center';
        ctx.textBaseline = 'alphabetic';
        const lineH = size * 1.4;
        const titleTop = H * 0.40 - (lines.length * lineH) / 2 + size * 0.82;
        lines.forEach((line, i) => ctx.fillText(line, W / 2, titleTop + i * lineH));

        // gold rule
        ctx.strokeStyle = plate.rule;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.moveTo(W / 2 - 27, H * 0.665);
        ctx.lineTo(W / 2 + 27, H * 0.665);
        ctx.stroke();

        // author in spaced caps
        let authorText = (cleanAuthor || 'Unknown').toUpperCase();
        if (authorText.length > 26) authorText = authorText.slice(0, 25) + '…';
        if ('letterSpacing' in ctx) ctx.letterSpacing = '2px';
        let aSize = 15;
        ctx.font = `500 ${aSize}px ${serif}`;
        while (ctx.measureText(authorText).width > W - 80 && aSize > 10) {
            aSize -= 1;
            ctx.font = `500 ${aSize}px ${serif}`;
        }
        ctx.globalAlpha = .78;
        ctx.fillStyle = plate.ink;
        ctx.fillText(authorText, W / 2, H * 0.72);
        ctx.globalAlpha = 1;

        // tiny imprint
        ctx.font = '600 9px -apple-system, "Segoe UI", "PingFang SC", sans-serif';
        if ('letterSpacing' in ctx) ctx.letterSpacing = '3px';
        ctx.globalAlpha = .55;
        ctx.fillStyle = plate.rule;
        ctx.fillText('BIBLIO', W / 2, H * 0.93);
        ctx.globalAlpha = 1;
        if ('letterSpacing' in ctx) ctx.letterSpacing = '0px';

        const dataUrl = canvas.toDataURL('image/jpeg', 0.9);
        if (this._coverCache.size > 400) this._coverCache.clear();
        this._coverCache.set(cacheKey, dataUrl);
        return dataUrl;
    }

    wrapText(ctx, text, maxWidth, lineHeight) {
        const words = text.split(' ');
        const lines = [];
        let currentLine = '';

        words.forEach(word => {
            const testLine = currentLine + (currentLine ? ' ' : '') + word;
            const metrics = ctx.measureText(testLine);

            if (metrics.width > maxWidth && currentLine) {
                lines.push(currentLine);
                currentLine = word;
            } else {
                currentLine = testLine;
            }
        });

        if (currentLine) {
            lines.push(currentLine);
        }

        return lines;
    }

    shadeColor(color, percent) {
        const num = parseInt(color.replace('#', ''), 16);
        const amt = Math.round(2.55 * percent);
        const R = Math.max(0, Math.min(255, (num >> 16) + amt));
        const G = Math.max(0, Math.min(255, (num >> 8 & 0x00FF) + amt));
        const B = Math.max(0, Math.min(255, (num & 0x0000FF) + amt));
        return '#' + (0x1000000 + (R << 16) + (G << 8) + B).toString(16).slice(1);
    }

    loadAndDisplayFormats(bookId) {
        const formatsContainer = document.getElementById('detailFormats');
        if (!formatsContainer) return;

        const book = this.selectedBook;
        if (book && book.formats && book.formats.length > 0) {
            formatsContainer.innerHTML = book.formats.map(format => `
                <a href="/api/libraries/${this.currentLibraryId}/books/${bookId}/formats/${format}"
                   target="_blank" rel="noopener" class="format-button" title="Download ${format.toUpperCase()}">
                    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                        stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M12 4v10M8 11l4 4 4-4M5 19h14" />
                    </svg>
                    ${format.toUpperCase()}
                </a>`).join('');
        } else {
            formatsContainer.innerHTML = '<span class="no-formats">No formats available</span>';
        }
    }

    updateSort() {
        this.sortMethod = document.getElementById('sortSelect').value;
        this.saveAppState();
        this.applyFilters();
    }

    setupEventListeners() {
        document.getElementById('searchInput').addEventListener('input', (e) => {
            this.searchTerm = e.target.value;

            // Show/hide clear button based on input value
            const clearBtn = document.getElementById('clearSearchBtn');
            if (clearBtn) {
                clearBtn.style.display = this.searchTerm ? 'flex' : 'none';
            }

            this.saveAppState();

            // Searching now hits the server, so debounce while typing
            clearTimeout(this.searchTimer);
            this.searchTimer = setTimeout(() => this.applyFilters(), 300);
        });

        // Setup splitter event listeners
        this.setupSplitters();

        // Setup responsive panel sizing
        this.setupResponsivePanelSizing();

        // Per-section filter search (authors / series / tags)
        ['authors', 'series', 'tags'].forEach(kind => {
            const input = document.getElementById(kind + 'Search');
            if (!input) return;
            let timer = null;
            input.addEventListener('input', () => {
                clearTimeout(timer);
                timer = setTimeout(() => this.renderFilterSection(kind), 140);
            });
            input.addEventListener('keydown', (e) => {
                if (e.key === 'Escape') {
                    input.value = '';
                    this.renderFilterSection(kind);
                    input.blur();
                }
                e.stopPropagation();
            });
            // Clicks inside the sidebar must not bubble into the drawer closer
            input.addEventListener('click', (e) => e.stopPropagation());
        });

        // The sidebar is a drawer at every width: close it after picking a filter.
        document.addEventListener('click', (e) => {
            const panel = document.querySelector('.left-panel');
            if (!panel || !panel.classList.contains('open')) return;
            if (e.target.closest('.filter-item') || e.target.closest('#librariesList')) {
                panel.classList.remove('open');
            }
        });

        // No focused book -> the details peek goes away on the next outside click.
        document.addEventListener('click', (e) => {
            const panel = document.querySelector('.right-panel');
            if (!panel || !panel.classList.contains('open')) return;
            const t = e.target;
            if (t.closest('.right-panel')) return;
            if (t.closest('.book-item') || t.closest('#booksTableBody')) return;
            if (t.closest('[data-fav-key]') || t.closest('.fav-item')) return;
            if (t.closest('#filtersToggle')) return;
            panel.classList.remove('open');
        });
    }

    clearSearch() {
        const searchInput = document.getElementById('searchInput');
        const clearBtn = document.getElementById('clearSearchBtn');

        // Clear the input and search term
        searchInput.value = '';
        this.searchTerm = '';

        // Hide clear button
        if (clearBtn) {
            clearBtn.style.display = 'none';
        }

        // Focus back on input
        searchInput.focus();

        // Reapply filters
        clearTimeout(this.searchTimer);
        this.displayedBooksCount = 0;
        this.saveAppState();
        this.applyFilters();
    }

    setupSplitters() {
        const splitterLeft = document.getElementById('splitterLeft');
        const splitterRight = document.getElementById('splitterRight');

        // Helper function to get X coordinate from mouse or touch event
        const getClientX = (e) => {
            return e.clientX !== undefined ? e.clientX : (e.touches && e.touches[0] ? e.touches[0].clientX : 0);
        };

        // Helper function to start resizing from either mouse or touch
        const startResize = (side, e) => {
            e.preventDefault();
            this.resizing = {
                side: side,
                startX: getClientX(e),
                startLeftWidth: document.querySelector('.left-panel').offsetWidth,
                startRightWidth: document.querySelector('.right-panel').offsetWidth,
                splitter: e.target
            };
            e.target.classList.add('active');
        };

        if (splitterLeft) {
            splitterLeft.addEventListener('mousedown', (e) => startResize('left', e));
            splitterLeft.addEventListener('touchstart', (e) => startResize('left', e), { passive: false });
        }

        if (splitterRight) {
            splitterRight.addEventListener('mousedown', (e) => startResize('right', e));
            splitterRight.addEventListener('touchstart', (e) => startResize('right', e), { passive: false });
        }

        document.addEventListener('mousemove', (e) => this.handleResize(e));
        document.addEventListener('touchmove', (e) => this.handleResize(e), { passive: false });
        document.addEventListener('mouseup', () => this.stopResizing());
        document.addEventListener('touchend', () => this.stopResizing());

        // Load saved pane sizes
        this.loadPaneSizes();
    }

    handleResize(e) {
        if (!this.resizing) return;

        // Get X coordinate from mouse or touch event
        const clientX = e.clientX !== undefined ? e.clientX : (e.touches && e.touches[0] ? e.touches[0].clientX : null);
        if (clientX === null) return;

        const leftPanel = document.querySelector('.left-panel');
        const rightPanel = document.querySelector('.right-panel');

        if (!leftPanel || !rightPanel) return;

        if (this.resizing.side === 'left') {
            const deltaX = clientX - this.resizing.startX;
            const newLeftWidth = Math.max(150, Math.min(500, this.resizing.startLeftWidth + deltaX));
            leftPanel.style.width = newLeftWidth + 'px';
            leftPanel.style.flex = `0 0 ${newLeftWidth}px`;
        } else if (this.resizing.side === 'right') {
            const deltaX = clientX - this.resizing.startX;
            const newRightWidth = Math.max(150, Math.min(500, this.resizing.startRightWidth - deltaX));
            rightPanel.style.width = newRightWidth + 'px';
            rightPanel.style.flex = `0 0 ${newRightWidth}px`;
        }
    }

    stopResizing() {
        if (this.resizing) {
            if (this.resizing.splitter) {
                this.resizing.splitter.classList.remove('active');
            }
            this.savePaneSizes();

            // Re-apply table column widths when panels are resized
            if (this.currentViewMode === 'table') {
                this.loadTableColumnWidths();
            }

            this.resizing = null;
        }
    }

    savePaneSizes() {
        const leftWidth = document.querySelector('.left-panel').offsetWidth;
        const rightWidth = document.querySelector('.right-panel').offsetWidth;

        const paneSizes = {
            leftWidth: leftWidth,
            rightWidth: rightWidth
        };

        localStorage.setItem('biblioPaneSizes', JSON.stringify(paneSizes));
    }

    loadPaneSizes() {
        // Panels are side-peek overlays sized by CSS; the old pane-dragging layout
        // (and its persisted widths) is retired.
        localStorage.removeItem('biblioPaneSizes');
    }

    setupResponsivePanelSizing() {
        // Nothing left to measure: both panels are absolutely positioned overlays.
    }

    adjustPanelsForViewport() {
        // No-op by design. CSS owns panel geometry now.
    }


    sendDiagnosticsToServer(diagnostics) {
        fetch('/api/diagnostics/client', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify(diagnostics)
        }).catch(err => {
            // Silently fail if server endpoint not available (no console logging to avoid clutter)
        });
    }

    // View Mode and Table Column Width Management
    setViewMode(mode) {
        this.currentViewMode = mode;
        this.saveViewPreferences();
        this.updateViewDisplay();
    }

    updateViewDisplay() {
        const booksGrid = document.getElementById('booksGrid');
        const booksTable = document.getElementById('booksTable');
        const gridRadio = document.querySelector('input[name="viewMode"][value="grid"]');
        const tableRadio = document.querySelector('input[name="viewMode"][value="table"]');
        const columnVisibilityItem = document.getElementById('columnVisibilityItem');
        const coverSizeItem = document.getElementById('coverSizeItem');

        if (this.currentViewMode === 'grid') {
            booksGrid.style.display = 'grid';
            booksTable.style.display = 'none';
            if (gridRadio) gridRadio.checked = true;
            if (columnVisibilityItem) columnVisibilityItem.style.display = 'none';
            if (coverSizeItem) coverSizeItem.style.display = 'flex';

            // Apply current cover size (set CSS custom properties)
            this.applyCoverSizeCSS();

            // Always reset displayed count when switching to grid mode to show current filters
            this.displayedBooksCount = 0;
            this.renderBooks();
            this.setupInfiniteScroll();
        } else {
            booksGrid.style.display = 'none';
            booksTable.style.display = 'flex';
            if (tableRadio) tableRadio.checked = true;
            if (columnVisibilityItem) columnVisibilityItem.style.display = 'flex';
            if (coverSizeItem) coverSizeItem.style.display = 'none';
            // Always reset displayed count when switching to table mode to show current filters
            this.displayedBooksCount = 0;
            this.renderBooksTable();
            this.setupTableInfiniteScroll();
        }
    }

    saveViewPreferences() {
        const preferences = {
            viewMode: this.currentViewMode,
            coverSize: this.coverSize,
            tableColumnWidths: this.tableColumnWidths,
            columnVisibility: this.columnVisibility
        };
        localStorage.setItem('biblioViewPreferences', JSON.stringify(preferences));
    }

    loadViewPreferences() {
        const preferences = localStorage.getItem('biblioViewPreferences');
        if (preferences) {
            try {
                const prefs = JSON.parse(preferences);
                if (prefs.viewMode) {
                    this.currentViewMode = prefs.viewMode;
                }
                if (prefs.coverSize && prefs.coverSize !== 120) {
                    this.coverSize = prefs.coverSize;
                }
                if (prefs.tableColumnWidths) {
                    this.tableColumnWidths = prefs.tableColumnWidths;
                }
                if (prefs.columnVisibility) {
                    this.columnVisibility = prefs.columnVisibility;
                }
            } catch (error) {
                console.error('Error loading view preferences:', error);
            }
        }

        // Sync checkboxes with loaded column visibility state
        this.syncColumnVisibilityCheckboxes();
    }

    syncColumnVisibilityCheckboxes() {
        const columns = ['title', 'authors', 'series', 'publisher', 'rating', 'pubdate'];
        columns.forEach(colName => {
            const checkbox = document.querySelector(`input[data-col="${colName}"]`);
            if (checkbox) {
                checkbox.checked = this.columnVisibility[colName];
            }
        });
    }

    toggleColumnVisibility(colName) {
        this.columnVisibility[colName] = !this.columnVisibility[colName];

        // If showing a column, ensure it has a valid width
        if (this.columnVisibility[colName]) {
            const currentWidth = this.tableColumnWidths[colName];
            if (!currentWidth || currentWidth <= 0) {
                // Assign default widths for columns
                const defaultWidths = {
                    title: 30,
                    authors: 20,
                    series: 15,
                    publisher: 15,
                    rating: 10,
                    pubdate: 10
                };
                this.tableColumnWidths[colName] = defaultWidths[colName] || 15;
            }
        }

        this.saveViewPreferences();
        this.updateColumnVisibility();

        // Update checkbox state
        const checkbox = document.querySelector(`input[data-col="${colName}"]`);
        if (checkbox) {
            checkbox.checked = this.columnVisibility[colName];
        }
    }

    applyCoverSizeCSS() {
        const size = this.coverSize;
        const grid = document.getElementById('booksGrid');
        if (grid) {
            // Calculate proportional spacing based on default 120px with 15px gap
            // Ratio: 15/120 = 0.125 for gap, 8/120 = 0.067 for margin
            const proportionalGap = Math.round(size * 0.125);
            const proportionalPadding = proportionalGap;
            const proportionalMargin = Math.round(size * 0.067);

            grid.style.setProperty('--cover-width', size + 'px');
            grid.style.setProperty('--item-gap', proportionalGap + 'px');
            grid.style.setProperty('--grid-padding', proportionalPadding + 'px');
            grid.style.setProperty('--cover-margin', proportionalMargin + 'px');

            // Calculate title font size and line clamp based on cover size
            const { fontSize, lineClamp } = this.calculateTitleMetrics(size);
            grid.style.setProperty('--title-font-size', fontSize + 'px');
            grid.style.setProperty('--title-lines', lineClamp.toString());

            // After CSS properties are applied, check if we need to load more books to fill screen
            setTimeout(() => this.fillScreenWithCovers(), 50);
        }

        // Update range slider and label
        const slider = document.getElementById('coverSizeSlider');
        const label = document.getElementById('coverSizeLabel');
        if (slider) slider.value = size;
        if (label) label.textContent = size;
    }

    fillScreenWithCovers() {
        const booksGrid = document.getElementById('booksGrid');
        if (!booksGrid || this.currentViewMode !== 'grid') return;

        const scrollHeight = booksGrid.scrollHeight;
        const clientHeight = booksGrid.clientHeight;

        // If there's vertical space available, keep appending: first from what is
        // already loaded, then by asking the server for the next page.
        if (scrollHeight > clientHeight) return;
        if (this.displayedBooksCount >= this.filteredBooks.length &&
            this.filteredBooks.length >= this.totalBooks) {
            return;
        }

        if (this.displayedBooksCount < this.filteredBooks.length) {
            this.renderBooks();
            setTimeout(() => this.fillScreenWithCovers(), 30);
        } else {
            this.loadMoreBooks().then(loaded => {
                if (loaded) setTimeout(() => this.fillScreenWithCovers(), 30);
            });
        }
    }

    fillScreenWithRows() {
        const booksTable = document.getElementById('booksTable');
        if (!booksTable || this.currentViewMode !== 'table') return;

        const scrollHeight = booksTable.scrollHeight;
        const clientHeight = booksTable.clientHeight;

        if (scrollHeight > clientHeight) return;
        if (this.displayedBooksCount >= this.filteredBooks.length &&
            this.filteredBooks.length >= this.totalBooks) {
            return;
        }

        if (this.displayedBooksCount < this.filteredBooks.length) {
            this.renderBooksTable();
            setTimeout(() => this.fillScreenWithRows(), 30);
        } else {
            this.loadMoreBooks().then(loaded => {
                if (loaded) setTimeout(() => this.fillScreenWithRows(), 30);
            });
        }
    }

    calculateTitleMetrics(coverWidth) {
        // Calculate dynamic font size: scale from 10px (at 50px) to 14px (at 250px)
        // Linear interpolation: fontSize = 10 + (coverWidth - 50) * (14 - 10) / (250 - 50)
        const fontSize = Math.max(10, Math.min(14, 10 + (coverWidth - 50) * 4 / 200));

        // Calculate line clamp based on cover width
        // At small sizes: 1 line, at medium: 2 lines, at large: 3 lines
        let lineClamp = 2;
        if (coverWidth < 80) {
            lineClamp = 1;
        } else if (coverWidth < 120) {
            lineClamp = 2;
        } else if (coverWidth < 180) {
            lineClamp = 2;
        } else if (coverWidth < 220) {
            lineClamp = 3;
        } else {
            lineClamp = 3;
        }

        return { fontSize: Math.round(fontSize * 10) / 10, lineClamp };
    }

    setCoverSize(sizeInPixels) {
        const size = parseInt(sizeInPixels);
        this.coverSize = size;
        this.applyCoverSizeCSS();
        this.saveViewPreferences();
    }

    updateColumnVisibility() {
        const table = document.getElementById('booksTableElement');
        if (!table) return;

        const headers = table.querySelectorAll('th.resizable-col');
        const columns = ['title', 'authors', 'series', 'publisher', 'rating', 'pubdate'];

        // Hide/show headers
        headers.forEach((th, index) => {
            const colName = columns[index];
            th.style.display = this.columnVisibility[colName] ? 'table-cell' : 'none';
        });

        // Hide/show cells in rows
        const rows = table.querySelectorAll('tbody tr');
        rows.forEach(row => {
            const cells = row.querySelectorAll('td');
            cells.forEach((td, index) => {
                const colName = columns[index];
                td.style.display = this.columnVisibility[colName] ? 'table-cell' : 'none';
            });
        });

        // Recalculate visible column percentages
        this.recalculateColumnWidths();
    }

    recalculateColumnWidths() {
        // Recalculate percentages based on visible columns only
        const visibleColumns = Object.entries(this.columnVisibility)
            .filter(([_, visible]) => visible)
            .map(([col, _]) => col);

        const table = document.getElementById('booksTableElement');
        if (!table || visibleColumns.length === 0) return;

        const headers = table.querySelectorAll('th.resizable-col');
        const columns = ['title', 'authors', 'series', 'publisher', 'rating', 'pubdate'];

        // Default widths for columns
        const defaultWidths = {
            title: 30,
            authors: 20,
            series: 15,
            publisher: 15,
            rating: 10,
            pubdate: 10
        };

        // Ensure all visible columns have valid widths
        visibleColumns.forEach(colName => {
            if (!this.tableColumnWidths[colName] || this.tableColumnWidths[colName] <= 0) {
                this.tableColumnWidths[colName] = defaultWidths[colName] || 15;
            }
        });

        // Calculate new percentages distributed among visible columns
        let totalVisiblePercentage = 0;
        headers.forEach((th, index) => {
            if (th.style.display !== 'none') {
                totalVisiblePercentage += this.tableColumnWidths[columns[index]] || 0;
            }
        });

        // Redistribute percentages
        headers.forEach((th, index) => {
            const colName = columns[index];
            if (this.columnVisibility[colName]) {
                const newPercent = totalVisiblePercentage > 0
                    ? (this.tableColumnWidths[colName] / totalVisiblePercentage) * 100
                    : (100 / visibleColumns.length);
                th.style.width = newPercent + '%';
            }
        });
    }

    renderBooksTable() {
        const tbody = document.getElementById('booksTableBody');
        const table = document.getElementById('booksTableElement');
        if (!tbody || !table) return;

        // Clear tbody on initial render
        if (this.displayedBooksCount === 0) {
            tbody.innerHTML = '';
        }

        // Calculate which books to display
        const startIndex = this.displayedBooksCount;
        const endIndex = Math.min(this.displayedBooksCount + this.booksPerPage, this.filteredBooks.length);
        const booksToDisplay = this.filteredBooks.slice(startIndex, endIndex);

        // Add new rows to table
        booksToDisplay.forEach(book => {
            const row = document.createElement('tr');
            row.dataset.bookId = book.id;
            row.dataset.libraryId = this.currentLibraryId;
            if (this.selectedBookId === book.id && this.selectedBookLibraryId === this.currentLibraryId) {
                row.classList.add('selected');
            }

            const favActive = this.isFavorite(this.currentLibraryId, book.id);
            row.innerHTML = `
                <td><button class="row-fav${favActive ? ' active' : ''}" data-fav-key="${this.favKey(this.currentLibraryId, book.id)}" title="Favorite" aria-label="Toggle favorite"><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20.4c-1-.7-8.5-5.4-10.2-9.6C.7 7.4 2.9 4 6.3 4c2 0 3.7 1 5.7 3 2-2 3.7-3 5.7-3 3.4 0 5.6 3.4 4.5 6.8-1.7 4.2-9.2 8.9-10.2 9.6Z"/></svg></button>${this.escapeHtml(book.title || '')}</td>
                <td>${this.escapeHtml((book.authors || []).map(author => this.formatAuthorName(author)).join(', '))}</td>
                <td>${this.escapeHtml(book.series && book.series_index ? `${book.series} #${book.series_index}` : (book.series || ''))}</td>
                <td>${this.escapeHtml((book.publisher || ''))}</td>
                <td>${book.rating ? this.renderStars(book.rating) : '<span style="color:var(--muted)">—</span>'}</td>
                <td>${book.pubdate ? new Date(book.pubdate).toLocaleDateString() : 'N/A'}</td>
            `;

            row.querySelector('.row-fav').addEventListener('click', (e) => {
                e.stopPropagation();
                this.toggleFavorite({
                    lib: this.currentLibraryId,
                    id: book.id,
                    title: book.title,
                    authors: book.authors || []
                });
            });

            row.addEventListener('click', (e) => {
                this.selectBook(book, this.currentLibraryId);
            });

            tbody.appendChild(row);
        });

        // Update displayed count
        this.displayedBooksCount = endIndex;
        this.tableRenderedForLibrary = this.currentLibraryId;

        // Load and apply saved column widths (only on first render)
        if (startIndex === 0) {
            this.loadTableColumnWidths();
            // Setup column resizing
            this.setupTableColumnResizing();
            // Setup resize observer for responsive column scaling
            this.setupTableResizeObserver();
        }

        // Apply column visibility
        this.updateColumnVisibility();

        // Setup infinite scroll on first render
        if (startIndex === 0 && this.displayedBooksCount === this.booksPerPage) {
            this.setupTableInfiniteScroll();
        }
    }

    setupTableInfiniteScroll() {
        const booksTable = document.getElementById('booksTable');
        if (!booksTable) return;

        // Remove existing listener if any
        booksTable.removeEventListener('scroll', this.handleTableScroll);

        // Add new scroll listener
        this.handleTableScroll = () => {
            this.trackChrome(booksTable.scrollTop);
            if (this.isLoadingMore) {
                return;
            }
            if (this.displayedBooksCount >= this.filteredBooks.length &&
                this.filteredBooks.length >= this.totalBooks) {
                return;
            }

            const scrollTop = booksTable.scrollTop;
            const scrollHeight = booksTable.scrollHeight;
            const clientHeight = booksTable.clientHeight;

            // Load more books when user scrolls to 80% of the way down
            if (scrollTop + clientHeight >= scrollHeight * 0.8) {
                if (this.displayedBooksCount < this.filteredBooks.length) {
                    this.renderBooksTable();
                } else {
                    this.loadMoreBooks();
                }
            }
        };

        booksTable.addEventListener('scroll', this.handleTableScroll);
    }

    setupTableColumnResizing() {
        const resizeHandles = document.querySelectorAll('.col-resize');

        resizeHandles.forEach((handle, index) => {
            // Helper function to get X coordinate from mouse or touch event
            const getClientX = (e) => {
                return e.clientX !== undefined ? e.clientX : (e.touches && e.touches[0] ? e.touches[0].clientX : 0);
            };

            // Helper function to start column resize
            const startResize = (e) => {
                e.preventDefault();
                e.stopPropagation();

                const th = handle.closest('th');
                const table = document.getElementById('booksTableElement');
                const startX = getClientX(e);
                const startWidth = th.offsetWidth;
                const startTableWidth = table.offsetWidth;
                const colName = th.getAttribute('data-col') || this.getColumnNameByIndex(index);

                const handleMouseMove = (e) => {
                    const deltaX = getClientX(e) - startX;
                    const newWidth = Math.max(50, startWidth + deltaX);
                    // Convert to percentage for responsive behavior
                    const percentage = (newWidth / startTableWidth) * 100;
                    th.style.width = percentage + '%';
                };

                const handleMouseUp = () => {
                    document.removeEventListener('mousemove', handleMouseMove);
                    document.removeEventListener('mouseup', handleMouseUp);
                    document.removeEventListener('touchmove', handleMouseMove);
                    document.removeEventListener('touchend', handleMouseUp);

                    // Save new column widths as percentages
                    this.saveTableColumnWidths();
                };

                document.addEventListener('mousemove', handleMouseMove);
                document.addEventListener('mouseup', handleMouseUp);
                document.addEventListener('touchmove', handleMouseMove, { passive: false });
                document.addEventListener('touchend', handleMouseUp);
            };

            handle.addEventListener('mousedown', startResize);
            handle.addEventListener('touchstart', startResize, { passive: false });
        });
    }

    getColumnNameByIndex(index) {
        const columns = ['title', 'authors', 'series', 'publisher', 'rating', 'pubdate'];
        return columns[index] || 'title';
    }

    saveTableColumnWidths() {
        const table = document.getElementById('booksTableElement');
        if (!table) return;

        const headers = table.querySelectorAll('th.resizable-col');
        const columns = ['title', 'authors', 'series', 'publisher', 'rating', 'pubdate'];

        headers.forEach((th, index) => {
            const colName = columns[index];
            const width = th.offsetWidth;
            const tableWidth = table.offsetWidth;
            const percentage = (width / tableWidth) * 100;
            this.tableColumnWidths[colName] = percentage;
        });

        this.saveViewPreferences();
    }

    loadTableColumnWidths() {
        const table = document.getElementById('booksTableElement');
        if (!table) return;

        const headers = table.querySelectorAll('th.resizable-col');
        const columns = ['title', 'authors', 'series', 'publisher', 'rating', 'pubdate'];

        // Default widths for columns
        const defaultWidths = {
            title: 30,
            authors: 20,
            series: 15,
            publisher: 15,
            rating: 10,
            pubdate: 10
        };

        headers.forEach((th, index) => {
            const colName = columns[index];
            let width = this.tableColumnWidths[colName];

            // Ensure width is valid
            if (!width || width <= 0) {
                width = defaultWidths[colName] || 15;
                this.tableColumnWidths[colName] = width;
            }

            // Always apply as percentage for responsive scaling
            th.style.width = width + '%';
        });
    }

    setupTableResizeObserver() {
        const booksTable = document.getElementById('booksTable');
        if (!booksTable) return;

        // Create resize observer to detect when panel is resized
        const resizeObserver = new ResizeObserver(() => {
            // Re-apply column widths when container is resized
            this.loadTableColumnWidths();
        });

        resizeObserver.observe(booksTable);
    }

    escapeHtml(text) {
        const map = {
            '&': '&amp;',
            '<': '&lt;',
            '>': '&gt;',
            '"': '&quot;',
            "'": '&#039;'
        };
        return text.replace(/[&<>"']/g, m => map[m]);
    }

    async refreshLibraries() {
        try {
            this.updateStatus('Refreshing libraries...');
            // Save current app state before refresh
            const savedState = this.loadAppState();

            const response = await fetch('/api/libraries/refresh', {
                method: 'POST'
            });
            const data = await response.json();

            if (data.success) {
                // Update libraries but preserve current state
                this.libraries = data.data || [];

                // Re-render libraries
                this.renderLibraries();

                // Recheck panel sizes on refresh
                this.adjustPanelsForViewport();

                // Restore previous selection if the library still exists
                if (savedState && savedState.currentLibraryId && this.libraries.some(lib => lib.id === savedState.currentLibraryId)) {
                    await this.selectLibrary(savedState.currentLibraryId);
                } else if (this.libraries.length > 0) {
                    await this.selectLibrary(this.libraries[0].id);
                }
                this.updateStatus('Libraries refreshed successfully');
            } else {
                console.error('Failed to refresh libraries:', data.error);
                this.updateStatus('Failed to refresh libraries: ' + (data.error || 'Unknown error'));
            }
        } catch (error) {
            console.error('Error refreshing libraries:', error);
            this.updateStatus('Error refreshing libraries');
        }
    }

    showAbout() {
        alert('Biblio - E-book Library Browser\n\nVersion 1.0.0\n\nA web-based application for browsing Calibre e-book libraries.\n\nBuilt with Rust (Actix-web) and modern web technologies');
    }

    formatNumber(n) {
        return Number(n || 0).toLocaleString('en-US');
    }

    renderStars(rating) {
        const full = Math.max(0, Math.min(5, Math.round((rating || 0) / 2)));
        return `<span class="stars">${'★'.repeat(full)}${'☆'.repeat(5 - full)}</span>`;
    }

    findAuthorId(name) {
        const hit = (this.authors || []).find(a => a.name === name);
        return hit ? hit.id : null;
    }

    findTagId(name) {
        const hit = (this.tags || []).find(t => t.name === name);
        return hit ? hit.id : null;
    }

    showToast(message) {
        const toast = document.getElementById('toast');
        if (!toast) return;
        toast.textContent = message;
        toast.classList.add('show');
        clearTimeout(this._toastTimer);
        this._toastTimer = setTimeout(() => toast.classList.remove('show'), 1800);
    }

    // Declarative description of the four sidebar filter lists.
    filterSections() {
        return {
            authors: {
                bodyId: 'authorsBody',
                searchId: 'authorsSearch',
                totalId: 'authorTotal',
                prefix: 'author',
                toggle: 'toggleAuthorFilter',
                data: () => this.authors,
                selected: () => this.selectedAuthors,
                label: (a) => this.formatAuthorName(a.name || ''),
                key: (a) => (a.sort || a.name || '').toLowerCase(),
                count: (a) => a.book_count || 0,
                group: true,
                empty: 'No authors found'
            },
            series: {
                bodyId: 'seriesBody',
                searchId: 'seriesSearch',
                totalId: 'seriesTotal',
                prefix: 'series',
                toggle: 'toggleSeriesFilter',
                data: () => this.series,
                selected: () => this.selectedSeries,
                label: (s) => s.name || '',
                key: (s) => (s.sort || s.name || '').toLowerCase(),
                count: (s) => s.book_count || 0,
                group: true,
                empty: 'No series found'
            },
            tags: {
                bodyId: 'tagsBody',
                searchId: 'tagsSearch',
                totalId: 'tagTotal',
                prefix: 'tag',
                toggle: 'toggleTagFilter',
                data: () => this.tags,
                selected: () => this.selectedTags,
                label: (t) => t.name || '',
                key: (t) => (t.name || '').toLowerCase(),
                count: (t) => t.book_count || 0,
                group: true,
                empty: 'No tags found'
            },
            formats: {
                bodyId: 'formatsBody',
                searchId: null,
                totalId: 'formatTotal',
                prefix: 'format',
                toggle: 'toggleFormatFilter',
                isFormat: true,
                data: () => (this.formats || []).map(f => ({ id: f, name: f })),
                selected: () => this.selectedFormats,
                label: (f) => String(f.name).toUpperCase(),
                key: (f) => String(f.name),
                count: (f) => (this.formatsCount || {})[f.name] || 0,
                group: false,
                empty: 'No formats found'
            }
        };
    }

    // Render one sidebar section. The whole 68k-author list is never built:
    // without a search term the list is grouped by initial (groups are filled
    // lazily), with one it becomes a capped, "show more" flat list.
    renderFilterSection(kind) {
        const cfg = this.filterSections()[kind];
        if (!cfg) return;
        const body = document.getElementById(cfg.bodyId);
        if (!body) return;

        const data = cfg.data() || [];
        const total = document.getElementById(cfg.totalId);
        if (total) total.textContent = data.length ? this.formatNumber(data.length) : '';

        const searchEl = cfg.searchId ? document.getElementById(cfg.searchId) : null;
        const q = searchEl && searchEl.value ? searchEl.value.trim().toLowerCase() : '';

        // Sort on a precomputed key: plain string compare is an order of
        // magnitude cheaper than 68k localeCompare calls.
        const keyed = data.map(it => [String(cfg.key(it)), it]);
        keyed.sort((a, b) => (a[0] < b[0] ? -1 : (a[0] > b[0] ? 1 : 0)));
        const sorted = keyed.map(pair => pair[1]);

        body.innerHTML = '';
        if (sorted.length === 0) {
            body.innerHTML = `<div class="fsec-empty">${cfg.empty}</div>`;
            return;
        }

        if (q) {
            const matches = sorted.filter(it => {
                const label = String(cfg.label(it)).toLowerCase();
                return label.includes(q) || String(cfg.key(it)).includes(q);
            });
            if (!matches.length) {
                body.innerHTML = `<div class="fsec-empty">No match for &ldquo;${this.escapeHtml(q)}&rdquo;</div>`;
                return;
            }
            body.appendChild(this.renderFlatFilterList(cfg, matches, 40));
            return;
        }

        if (cfg.group && sorted.length > 80) {
            this.renderGroupedFilterList(cfg, sorted, body);
            return;
        }

        body.appendChild(this.renderFlatFilterList(cfg, sorted, 60));
    }

    renderFlatFilterList(cfg, items, pageSize) {
        const wrap = document.createElement('div');
        const chunk = document.createElement('div');
        wrap.appendChild(chunk);

        let next = 0;
        const more = document.createElement('button');
        more.type = 'button';
        more.className = 'fsec-more';
        more.addEventListener('click', (e) => {
            e.stopPropagation();
            appendMore();
        });

        const appendMore = () => {
            const end = Math.min(next + pageSize, items.length);
            for (; next < end; next++) {
                chunk.appendChild(this.makeFilterItem(cfg, items[next]));
            }
            if (next >= items.length) {
                more.remove();
            } else {
                more.textContent = `Show more · ${this.formatNumber(items.length - next)} left`;
            }
        };

        wrap.appendChild(more);
        appendMore();
        return wrap;
    }

    renderGroupedFilterList(cfg, items, body) {
        const groups = {};
        items.forEach(it => {
            const letter = this.getFirstLetterFromString(String(cfg.key(it)));
            (groups[letter] = groups[letter] || []).push(it);
        });

        const letters = Object.keys(groups).sort((a, b) => {
            if (a === '#') return 1;
            if (b === '#') return -1;
            return a < b ? -1 : (a > b ? 1 : 0);
        });

        letters.forEach((letter, index) => {
            const wrap = document.createElement('div');
            wrap.className = 'filter-group';

            const header = document.createElement('div');
            header.className = 'filter-group-header';
            header.innerHTML = `
                <span>${this.escapeHtml(letter)} · ${this.formatNumber(groups[letter].length)}</span>
                <span class="g-caret" style="transition:transform .18s">
                    <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                        stroke-width="2.6" stroke-linecap="round"><path d="M6 9l6 6 6-6" /></svg>
                </span>`;

            const content = document.createElement('div');
            content.className = 'filter-group-content';
            content.style.display = 'none';

            let built = false;
            const toggle = () => {
                if (!built) {
                    content.appendChild(this.renderFlatFilterList(cfg, groups[letter], 50));
                    built = true;
                }
                const open = content.style.display === 'none';
                content.style.display = open ? 'block' : 'none';
                const caret = header.querySelector('.g-caret');
                if (caret) caret.style.transform = open ? 'rotate(180deg)' : 'none';
            };
            header.addEventListener('click', toggle);

            wrap.appendChild(header);
            wrap.appendChild(content);

            // Auto-open the first group only when it is small enough to scan
            if (index === 0 && groups[letter].length <= 40) {
                toggle();
            }
            body.appendChild(wrap);
        });
    }

    makeFilterItem(cfg, item) {
        const row = document.createElement('div');
        row.className = 'filter-item';

        const inputId = `${cfg.prefix}_${item.id}`;
        const keyValue = cfg.isFormat ? item.id : parseInt(item.id, 10);

        const box = document.createElement('input');
        box.type = 'checkbox';
        box.id = inputId;
        box.value = String(item.id);
        box.checked = cfg.selected().has(keyValue);
        box.addEventListener('change', () => this[cfg.toggle](item.id));

        const label = document.createElement('label');
        label.htmlFor = inputId;
        const count = cfg.count(item);
        label.innerHTML = `
            <span class="fx-name">${this.escapeHtml(cfg.label(item))}</span>
            <span class="fx-count">${count ? this.formatNumber(count) : ''}</span>`;

        row.appendChild(box);
        row.appendChild(label);
        return row;
    }

    // ------------------------------------------------------------------
    // Favorites — kept in localStorage; the Calibre library stays read-only.
    // ------------------------------------------------------------------
    favKey(libId, bookId) {
        return `${libId}:${bookId}`;
    }

    loadFavorites() {
        try {
            const raw = localStorage.getItem('biblioFavorites');
            this.favorites = raw ? JSON.parse(raw) : [];
        } catch (e) {
            this.favorites = [];
        }
        if (!Array.isArray(this.favorites)) this.favorites = [];
    }

    saveFavorites() {
        try {
            localStorage.setItem('biblioFavorites', JSON.stringify(this.favorites.slice(0, 500)));
        } catch (e) { }
    }

    isFavorite(libId, bookId) {
        const key = this.favKey(libId || this.currentLibraryId, bookId);
        return (this.favorites || []).some(f => this.favKey(f.lib, f.id) === key);
    }

    toggleFavorite(meta) {
        const lib = meta.lib || this.currentLibraryId;
        const key = this.favKey(lib, meta.id);
        const idx = (this.favorites || []).findIndex(f => this.favKey(f.lib, f.id) === key);
        if (idx >= 0) {
            this.favorites.splice(idx, 1);
        } else {
            this.favorites.unshift({
                lib: lib,
                id: meta.id,
                title: meta.title || '',
                authors: (meta.authors || []).slice(0, 3),
                ts: Date.now()
            });
        }
        this.saveFavorites();
        this.refreshFavoriteUI(lib, meta.id);
        this.renderFavorites();
        this.showToast(idx >= 0 ? 'Removed from favorites' : 'Added to favorites');
    }

    refreshFavoriteUI(lib, id) {
        const key = this.favKey(lib, id);
        const active = this.isFavorite(lib, id);
        document.querySelectorAll('[data-fav-key]').forEach(btn => {
            if (btn.dataset.favKey === key) btn.classList.toggle('active', active);
        });
        if (this.selectedBook && this.selectedBook.id === id && this.selectedBookLibraryId === lib) {
            this.syncDetailFavButton();
        }
    }

    syncDetailFavButton() {
        const btn = document.getElementById('detailFav');
        const label = document.getElementById('detailFavText');
        if (!btn) return;
        const active = !!(this.selectedBook && this.isFavorite(this.selectedBookLibraryId, this.selectedBook.id));
        btn.classList.toggle('active', active);
        if (label) label.textContent = active ? 'In favorites' : 'Add to favorites';
    }

    toggleFavoriteFromDetails() {
        if (!this.selectedBook) return;
        this.toggleFavorite({
            lib: this.selectedBookLibraryId || this.currentLibraryId,
            id: this.selectedBook.id,
            title: this.selectedBook.title,
            authors: this.selectedBook.authors || []
        });
    }

    renderFavorites() {
        const body = document.getElementById('favoritesBody');
        const total = document.getElementById('favTotal');
        if (!body) return;
        const list = this.favorites || [];
        if (total) total.textContent = list.length ? String(list.length) : '';
        body.innerHTML = '';
        if (!list.length) {
            body.innerHTML = '<div class="fsec-empty">No favorites yet — tap the heart on a book.</div>';
            return;
        }
        list.slice(0, 200).forEach(f => {
            const row = document.createElement('div');
            row.className = 'fav-item';
            row.innerHTML = `
                <img class="fav-thumb" loading="lazy" alt="" src="/api/libraries/${f.lib}/books/${f.id}/cover">
                <div class="fav-txt">
                    <span class="fav-title">${this.escapeHtml(f.title || 'Untitled')}</span>
                    <span class="fav-auth">${this.escapeHtml((f.authors || [])[0] || '')}</span>
                </div>
                <button class="icon-btn fav-remove" title="Remove favorite" aria-label="Remove favorite">✕</button>`;
            row.addEventListener('click', () => this.openFavorite(f.lib, f.id));
            row.querySelector('.fav-remove').addEventListener('click', (e) => {
                e.stopPropagation();
                this.toggleFavorite({ lib: f.lib, id: f.id, title: f.title, authors: f.authors });
            });
            body.appendChild(row);
        });
    }

    async openFavorite(libId, bookId) {
        try {
            const res = await fetch(`/api/libraries/${libId}/books/${bookId}`);
            const data = await res.json();
            if (!data || !data.success || !data.data) return;
            if (libId !== this.currentLibraryId) await this.selectLibrary(libId);
            this.selectBook(data.data, libId);
        } catch (e) {
            console.warn('Could not open favorite:', e);
        }
    }

    clearAllFilters() {
        this.searchTerm = '';
        const searchInput = document.getElementById('searchInput');
        if (searchInput) searchInput.value = '';
        const clearBtn = document.getElementById('clearSearchBtn');
        if (clearBtn) clearBtn.style.display = 'none';

        this.selectedAuthors.clear();
        this.selectedTags.clear();
        this.selectedSeries.clear();
        this.selectedFormats.clear();

        this.updateAuthorCount();
        this.updateTagCount();
        this.updateSeriesCount();
        this.updateFormatCount();
        ['authors', 'series', 'tags', 'formats'].forEach(kind => this.renderFilterSection(kind));

        this.displayedBooksCount = 0;
        this.saveAppState();
        this.applyFilters();
    }

    // Removable chips for everything currently narrowing the list.
    renderActiveChips() {
        const box = document.getElementById('activeChips');
        if (box) {
            box.innerHTML = '';

            const addChip = (kind, text, onRemove) => {
                const chip = document.createElement('button');
                chip.type = 'button';
                chip.className = 'chip';
                chip.title = 'Remove filter';
                const k = document.createElement('span');
                k.className = 'chip-kind';
                k.textContent = kind;
                const t = document.createElement('span');
                t.textContent = text;
                const x = document.createElement('span');
                x.className = 'chip-x';
                x.innerHTML = '<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>';
                chip.appendChild(k);
                chip.appendChild(t);
                chip.appendChild(x);
                chip.addEventListener('click', onRemove);
                box.appendChild(chip);
            };

            if (this.searchTerm) {
                addChip('search', `“${this.searchTerm}”`, () => this.clearSearch());
            }
            this.selectedAuthors.forEach(id => {
                const a = (this.authors || []).find(x => x.id === id);
                addChip('author', a ? this.formatAuthorName(a.name) : `#${id}`, () => this.toggleAuthorFilter(id));
            });
            this.selectedSeries.forEach(id => {
                const s = (this.series || []).find(x => x.id === id);
                addChip('series', s ? s.name : `#${id}`, () => this.toggleSeriesFilter(id));
            });
            this.selectedTags.forEach(id => {
                const t = (this.tags || []).find(x => x.id === id);
                addChip('tag', t ? t.name : `#${id}`, () => this.toggleTagFilter(id));
            });
            this.selectedFormats.forEach(f => {
                addChip('format', String(f).toUpperCase(), () => this.toggleFormatFilter(f));
            });

            if (!box.children.length) {
                box.innerHTML = '';
            } else {
                const clearAll = document.createElement('button');
                clearAll.type = 'button';
                clearAll.className = 'chip chip-clear';
                clearAll.textContent = 'Clear all';
                clearAll.addEventListener('click', () => this.clearAllFilters());
                box.appendChild(clearAll);
            }
        }

        const countEl = document.getElementById('toolbarCount');
        if (countEl) {
            const total = this.totalBooks || 0;
            const filtered = !!(this.searchTerm || this.selectedAuthors.size ||
                this.selectedTags.size || this.selectedSeries.size || this.selectedFormats.size);
            countEl.innerHTML = `<strong>${this.formatNumber(total)}</strong> ${filtered ? (total === 1 ? 'match' : 'matches') : 'books'}`;
        }
    }

    updateStatus(message) {
        document.getElementById('statusMessage').textContent = message;
    }
}

// Initialize the application when the DOM is ready
const app = new BiblioApp();

document.addEventListener('DOMContentLoaded', () => {
    app.init();
});
