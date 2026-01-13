GLOBAL_STYLESHEET = """
    QMainWindow { background-color: #131314; color: #E3E3E3; font-family: "Segoe UI", sans-serif; }
    QWidget { font-size: 10pt; color: #E3E3E3; }
    
    /* Panels & Frames */
    QFrame[class="Panel"] { background-color: #1E1F20; border-radius: 8px; border: 1px solid #444746; }
    QTabWidget::pane { border: 1px solid #444746; background: #1E1F20; }
    QTabBar::tab { background: #131314; color: #C4C7C5; padding: 8px 16px; border-top-left-radius: 4px; border-top-right-radius: 4px; margin-right: 2px; }
    QTabBar::tab:selected { background: #A8C7FA; color: #000000; font-weight: bold; }
    
    /* Inputs */
    QLineEdit, QSpinBox, QDoubleSpinBox, QComboBox { background-color: #2D2E31; border: 1px solid #444746; border-radius: 6px; padding: 5px; color: #F1F1F1; selection-background-color: #A8C7FA; selection-color: #000000; }
    QComboBox { padding-right: 20px; }

    /* Dropdown Menus (QComboBox Popup) */
    QComboBox QAbstractItemView {
        background-color: #1E1F20;
        border: 1px solid #444746;
        selection-background-color: #A8C7FA;
        selection-color: #000000;
        outline: none;
    }
    QComboBox QAbstractItemView::item {
        background-color: #1E1F20;
        color: #E3E3E3;
    }
    QComboBox QAbstractItemView::item:selected {
        background-color: #A8C7FA;
        color: #000000;
    }
    
    /* Dropdown Scrollbar */
    QComboBox QAbstractItemView QScrollBar:vertical { background-color: #1E1F20; width: 8px; }
    QComboBox QAbstractItemView QScrollBar::handle:vertical { background-color: #444746; border-radius: 4px; }
    QComboBox QAbstractItemView QScrollBar::add-line:vertical, QComboBox QAbstractItemView QScrollBar::sub-line:vertical { height: 0px; }

    /* Dropdown Arrow */
    QComboBox::drop-down { subcontrol-origin: padding; subcontrol-position: top right; width: 20px; border-left-width: 0px; }
    QComboBox::down-arrow { image: url(data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxMiIgaGVpZ2h0PSIxMiIgdmlld0JveD0iMCAwIDEyIDEyIj48cGF0aCBmaWxsPSIjRDRERUU5IiBkPSJNMiA0bDQgNCA0LTRoLTh6Ii8+PC9zdmc+); width: 12px; height: 12px; }

    /* Context Menus */
    QMenu { background-color: #1E1F20; color: #E3E3E3; border: 1px solid #444746; }
    QMenu::item { padding: 6px 24px; background-color: transparent; }
    QMenu::item:selected { background-color: #A8C7FA; color: #000000; }
    
    /* Buttons */
    QPushButton { background-color: #2D2E31; border: 1px solid #444746; border-radius: 6px; padding: 8px; color: #E3E3E3; font-weight: bold; }
    QPushButton:hover { background-color: #444746; }
    QPushButton:pressed { background-color: #A8C7FA; color: #000000; }
    QPushButton:checked { background-color: #A8C7FA; color: #000000; }
    QPushButton.Danger { background-color: #F28B82; color: #000000; }
    QPushButton.Danger:hover { background-color: #E57373; }
    QPushButton.Success { background-color: #81C995; color: #000000; }
    QPushButton.Success:hover { background-color: #66BB6A; }
    
    /* SpinBox Buttons */
    QPushButton.SpinBtn { background-color: #2D2E31; border-radius: 4px; font-size: 14px; border: 1px solid #444746; }
    QPushButton.SpinBtn:hover { background-color: #A8C7FA; color: #000000; }
    
    /* Tree/List */
    QTreeWidget { background-color: #1E1F20; border: none; font-family: "Consolas", monospace; font-size: 9pt; alternate-background-color: #2D2E31; color: #E3E3E3; }
    QHeaderView::section { background-color: #131314; color: #C4C7C5; padding: 6px; border: none; font-weight: bold; }
    
    /* Scrollbars */
    QScrollBar:vertical { background: #131314; width: 10px; }
    QScrollBar::handle:vertical { background: #444746; border-radius: 5px; }
    QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0px; }
    
    /* Custom Labels */
    QLabel.Header { font-size: 14pt; font-weight: bold; color: #F1F1F1; }
    QLabel.SubHeader { font-size: 11pt; font-weight: bold; color: #A8C7FA; }
    QLabel.Price { font-family: "Consolas", monospace; font-size: 20pt; font-weight: bold; color: #F1F1F1; }
    QLabel.StatusTag { padding: 4px 8px; border-radius: 4px; font-weight: bold; font-size: 9pt; }
    QLabel.HeaderLastSig { font-size: 9pt; color: #FDD663; font-family: Consolas; }
"""