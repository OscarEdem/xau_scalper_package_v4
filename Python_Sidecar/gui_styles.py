GLOBAL_STYLESHEET = """
    QMainWindow { background-color: #2E3440; color: #D8DEE9; font-family: "Segoe UI", sans-serif; }
    QWidget { font-size: 10pt; color: #D8DEE9; }
    
    /* Panels & Frames */
    QFrame.Panel { background-color: #3B4252; border-radius: 6px; border: 1px solid #434C5E; }
    QTabWidget::pane { border: 1px solid #434C5E; background: #3B4252; }
    QTabBar::tab { background: #2E3440; color: #D8DEE9; padding: 8px 16px; border-top-left-radius: 4px; border-top-right-radius: 4px; margin-right: 2px; }
    QTabBar::tab:selected { background: #88C0D0; color: #2E3440; font-weight: bold; }
    
    /* Inputs */
    QLineEdit, QSpinBox, QDoubleSpinBox, QComboBox { background-color: #434C5E; border: 1px solid #4C566A; border-radius: 4px; padding: 5px; color: #ECEFF4; selection-background-color: #88C0D0; }
    
    /* Buttons */
    QPushButton { background-color: #4C566A; border: none; border-radius: 4px; padding: 8px; color: #ECEFF4; font-weight: bold; }
    QPushButton:hover { background-color: #5E81AC; }
    QPushButton:pressed { background-color: #81A1C1; }
    QPushButton:checked { background-color: #88C0D0; color: #2E3440; }
    QPushButton.Danger { background-color: #BF616A; }
    QPushButton.Danger:hover { background-color: #D08770; }
    QPushButton.Success { background-color: #A3BE8C; color: #2E3440; }
    QPushButton.Success:hover { background-color: #B5D19E; }
    
    /* SpinBox Buttons */
    QPushButton.SpinBtn { background-color: #4C566A; border-radius: 2px; font-size: 14px; }
    QPushButton.SpinBtn:hover { background-color: #88C0D0; color: #2E3440; }
    
    /* Tree/List */
    QTreeWidget { background-color: #3B4252; border: none; font-family: "Consolas", monospace; font-size: 9pt; alternate-background-color: #434C5E; }
    QHeaderView::section { background-color: #2E3440; color: #D8DEE9; padding: 6px; border: none; font-weight: bold; }
    
    /* Scrollbars */
    QScrollBar:vertical { background: #2E3440; width: 10px; }
    QScrollBar::handle:vertical { background: #4C566A; border-radius: 5px; }
    QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0px; }
    
    /* Custom Labels */
    QLabel.Header { font-size: 14pt; font-weight: bold; color: #ECEFF4; }
    QLabel.SubHeader { font-size: 11pt; font-weight: bold; color: #88C0D0; }
    QLabel.Price { font-family: "Consolas", monospace; font-size: 20pt; font-weight: bold; color: #ECEFF4; }
    QLabel.StatusTag { padding: 4px 8px; border-radius: 4px; font-weight: bold; font-size: 9pt; }
    QLabel.HeaderLastSig { font-size: 9pt; color: #EBCB8B; font-family: Consolas; }
"""