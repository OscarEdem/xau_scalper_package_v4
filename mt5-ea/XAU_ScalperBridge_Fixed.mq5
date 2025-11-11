// XAU_ScalperBridge.mq5 v4.2 - Merged and Corrected
// Combines advanced UI with robust indicator handling.
#property strict
#property version   "4.2"
#include <Trade\Trade.mqh>

#property description "Calls an external server for XAU/USD scalping signals with an advanced dashboard."

// --- EA Inputs ---
input string ServerUrl = "http://127.0.0.1:3000/eval";
input double RiskPercent = 0.5;
input int    NumCloses = 80;
input double MaxSpreadPoints = 160;
input ulong  MagicNumber = 1337;

// --- Strategy Parameters (from best backtest) ---
input int    RsiPeriod = 16;
input int    EmaFastPeriod = 5;
input int    EmaSlowPeriod = 50;
input int    AtrPeriod = 14;
input double SlAtrMultiplier = 1.0;
input double TpAtrMultiplier = 1.5;

// --- Trailing Stop Inputs ---
input bool   UseTrailingStop = true;
input double TrailingStopATRMlt = 1.0; // From best backtest result
input int    TrailingATRPeriod = 14;

// --- Global Variables ---
CTrade trade;
char post_data[];
char result[];
string result_headers;
int    atr_handle; // Handle for the ATR indicator for trailing stops

// --- Function Prototypes ---
void ClosePositions(ENUM_POSITION_TYPE direction);
void CreatePanel(const long chart_ID, const string name, const int x, const int y, const int width, const int height, const color bg_color, const color border_color);
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color);
void UpdateDashboard(string action, string reason, string rsi, string ema_fast, string ema_slow, string atr, string tp, string sl, double current_spread, double current_balance, double current_pl);
double CalculateLotSize(double stop_loss_pips, string symbol, double point_value);
string GetJsonValue(string json, string key, bool is_string);
void ManageTrailingStops(); // Uses internal ATR handle

int OnInit() {
  // Make sure the terminal is configured to allow WebRequest
  trade.SetExpertMagicNumber(MagicNumber);
  // Go to Tools -> Options -> Expert Advisors and add the ServerUrl
  Print("XAU Scalper Bridge initialized. Server URL: ", ServerUrl);
  
  // Create ATR indicator handle for trailing stop
  atr_handle = iATR(_Symbol, PERIOD_M1, TrailingATRPeriod);
  if(atr_handle == INVALID_HANDLE) {
    Print("Failed to create ATR indicator handle. Error: ", GetLastError());
    return(INIT_FAILED);
  }
  
  return(INIT_SUCCEEDED);
}

void OnDeinit(const int reason) {
  // Clean up all graphical objects created by this EA
  ObjectsDeleteAll(0, "XauBridge_");
  IndicatorRelease(atr_handle); // Release the indicator handle
  ChartRedraw();
}

void OnTick(){
  static datetime last_bar=0;
  MqlRates rates[];

  // Run trailing stop on every tick
  if(UseTrailingStop) {
    ManageTrailingStops();
  }

  // Only run the rest of the logic on a new bar
  if(CopyRates(_Symbol, PERIOD_M1, 0, 1, rates) < 1) return;
  if(rates[0].time == last_bar) {
    return;
  }
  last_bar = rates[0].time;

  // --- Pre-trade checks ---
  double ask=SymbolInfoDouble(_Symbol, SYMBOL_ASK);
  double bid=SymbolInfoDouble(_Symbol, SYMBOL_BID);
  double point_val = SymbolInfoDouble(_Symbol, SYMBOL_POINT);
  double spread_raw = ask - bid;
  double spread_pts = spread_raw / point_val;
  
  // --- Calculate current P/L for any open position ---
  double current_pl = 0.0;
  for(int i = PositionsTotal() - 1; i >= 0; i--) {
    if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber) {
      current_pl = PositionGetDouble(POSITION_PROFIT);
      // We found our position, no need to loop further
      break; 
    }
  }
  
  PrintFormat("Spread Check: Raw Spread=%.5f, Point=%.5f, Spread Points=%.2f", spread_raw, point_val, spread_pts);
  
  if(spread_pts > MaxSpreadPoints) {
    Print("Spread is too high: ", spread_pts, " points. Skipping.");
    // Still update dashboard to show high spread
    UpdateDashboard("hold", "Spread too high", "-", "-", "-", "-", "-", "-", spread_pts, AccountInfoDouble(ACCOUNT_BALANCE), current_pl);
    return;
  }

  // --- Prepare data for server ---
  if(CopyRates(_Symbol, PERIOD_M1, 0, NumCloses, rates) < NumCloses) {
    Print("Could not get enough bar data. Need ", NumCloses, " bars.");
    return;
  }

  // Build the JSON payload
  string closes_str = "";
  string highs_str = "";
  string lows_str = "";

  for(int i = 0; i < ArraySize(rates); i++) {
    closes_str += DoubleToString(rates[i].close, _Digits);
    highs_str += DoubleToString(rates[i].high, _Digits);
    lows_str += DoubleToString(rates[i].low, _Digits);
    if(i < ArraySize(rates) - 1) {
      closes_str += ",";
      highs_str += ",";
      lows_str += ",";
    }
  }

  string json_payload = StringFormat(
    "{\"symbol\":\"%s\",\"timeframe\":\"M1\",\"closes\":[%s],\"highs\":[%s],\"lows\":[%s],"
    "\"rsi_period\":%d,\"ema_fast\":%d,\"ema_slow\":%d,\"atr_period\":%d,"
    "\"sl_atr_multiplier\":%.1f,\"tp_atr_multiplier\":%.1f}",
    _Symbol,
    closes_str, highs_str, lows_str,
    RsiPeriod, EmaFastPeriod, EmaSlowPeriod, AtrPeriod,
    SlAtrMultiplier, TpAtrMultiplier
  );

  // --- Call the Rust server ---
  ResetLastError();
  StringToCharArray(json_payload, post_data);
  int res = WebRequest("POST", ServerUrl, "Content-Type: application/json", 5000, post_data, result, result_headers);

  string action = "hold";
  string reason = "N/A";
  string rsi_val = "-";
  string ema_fast = "-";
  string ema_slow = "-";
  string tp_pips = "-";
  string sl_pips = "-";
  string atr_val_str = "-";

  if(res == -1) {
    Print("WebRequest failed. Error code: ", GetLastError());
    reason = "Request Failed";
  } else if(res != 200) {
    Print("Server returned non-200 status: ", res);
    Print("Server response: ", CharArrayToString(result));
    reason = "Server Error " + IntegerToString(res);
  } else {
    // --- Process server response ---
    string response_str = CharArrayToString(result);
    action = GetJsonValue(response_str, "action_advice", true);
    reason = GetJsonValue(response_str, "reason", true);
    rsi_val = GetJsonValue(response_str, "rsi", false);
    ema_fast = GetJsonValue(response_str, "ema_fast_last", false);
    ema_slow = GetJsonValue(response_str, "ema_slow_last", false);
    tp_pips = GetJsonValue(response_str, "tp_pips", false);
    sl_pips = GetJsonValue(response_str, "sl_pips", false);
    atr_val_str = GetJsonValue(response_str, "atr", false);
    
    PrintFormat("Server response: action=%s, reason=%s, rsi=%s, ema_fast=%s, ema_slow=%s, tp_pips=%s, sl_pips=%s, atr=%s",
                action, reason, rsi_val, ema_fast, ema_slow, tp_pips, sl_pips, atr_val_str);
  }

  // --- Display graphical dashboard on chart ---
  UpdateDashboard(action, reason, rsi_val, ema_fast, ema_slow, atr_val_str, tp_pips, sl_pips, spread_pts, AccountInfoDouble(ACCOUNT_BALANCE), current_pl);

  // --- Trade Execution ---
  double sl = StringToDouble(sl_pips);
  double tp = StringToDouble(tp_pips);

  if(action == "buy") {
    ClosePositions(POSITION_TYPE_SELL);
    if(PositionsTotal() == 0) {
      double lot_size = CalculateLotSize(sl, _Symbol, _Point);
      double stop_loss_price = bid - sl * _Point * 10.0;
      double take_profit_price = bid + tp * _Point * 10.0;
      if(lot_size > 0) trade.Buy(lot_size, _Symbol, bid, stop_loss_price, take_profit_price, "XAU Scalper Bridge BUY");
    }
  } else if(action == "sell") {
    ClosePositions(POSITION_TYPE_BUY);
    if(PositionsTotal() == 0) {
      double lot_size = CalculateLotSize(sl, _Symbol, _Point);
      double stop_loss_price = ask + sl * _Point * 10.0;
      double take_profit_price = ask - tp * _Point * 10.0;
      if(lot_size > 0) trade.Sell(lot_size, _Symbol, ask, stop_loss_price, take_profit_price, "XAU Scalper Bridge SELL");
    }
  }
}

// --- Trade Management Functions ---

void ClosePositions(ENUM_POSITION_TYPE direction) {
  for(int i = PositionsTotal() - 1; i >= 0; i--) {
    if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber && PositionGetInteger(POSITION_TYPE) == direction) {
      trade.PositionClose(PositionGetTicket(i));
    }
  }
}

void ManageTrailingStops() {
  // Get the latest ATR value from the handle created in OnInit
  double atr_buffer[];
  if(CopyBuffer(atr_handle, 0, 0, 1, atr_buffer) < 1) {
    Print("Could not get ATR value for trailing stop.");
    return;
  }
  double current_atr = atr_buffer[0];
  if(current_atr <= 0) return;

  // Loop through all open positions
  for(int i = PositionsTotal() - 1; i >= 0; i--) {
    ulong ticket = PositionGetTicket(i);
    if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber) {
      
      double entry_price = PositionGetDouble(POSITION_PRICE_OPEN);
      double current_sl = PositionGetDouble(POSITION_SL);
      double current_tp = PositionGetDouble(POSITION_TP);
      
      double new_sl = 0;
      
      // --- Logic for a LONG position ---
      if(PositionGetInteger(POSITION_TYPE) == POSITION_TYPE_BUY) {
        double current_bid = SymbolInfoDouble(_Symbol, SYMBOL_BID);
        new_sl = current_bid - (current_atr * TrailingStopATRMlt);
        if(new_sl > entry_price && new_sl > current_sl) {
          if(!trade.PositionModify(ticket, new_sl, current_tp)) {
            Print("Error modifying position #", ticket, " for trailing stop. Code: ", GetLastError());
          }
        }
      }
      
      // --- Logic for a SHORT position ---
      else if(PositionGetInteger(POSITION_TYPE) == POSITION_TYPE_SELL) {
        double current_ask = SymbolInfoDouble(_Symbol, SYMBOL_ASK);
        new_sl = current_ask + (current_atr * TrailingStopATRMlt);
        if(new_sl < entry_price && (current_sl == 0 || new_sl < current_sl)) { // Corrected logic for short SL
          if(!trade.PositionModify(ticket, new_sl, current_tp)) {
            Print("Error modifying position #", ticket, " for trailing stop. Code: ", GetLastError());
          }
        }
      }
    }
  }
}

// --- Graphical Dashboard Functions ---

void CreatePanel(const long chart_ID, const string name, const int x, const int y, const int width, const int height, const color bg_color) {
  if(ObjectFind(chart_ID, name) < 0) {
    ObjectCreate(chart_ID, name, OBJ_RECTANGLE_LABEL, 0, 0, 0);
    ObjectSetInteger(chart_ID, name, OBJPROP_XDISTANCE, x);
    ObjectSetInteger(chart_ID, name, OBJPROP_YDISTANCE, y);
    ObjectSetInteger(chart_ID, name, OBJPROP_XSIZE, width);
    ObjectSetInteger(chart_ID, name, OBJPROP_YSIZE, height);
    ObjectSetInteger(chart_ID, name, OBJPROP_CORNER, CORNER_LEFT_LOWER);
    ObjectSetInteger(chart_ID, name, OBJPROP_BGCOLOR, (long)bg_color);
    ObjectSetInteger(chart_ID, name, OBJPROP_SELECTABLE, false);
    ObjectSetInteger(chart_ID, name, OBJPROP_BACK, true);
  }
}
void CreatePanel(const long chart_ID, const string name, const int x, const int y, const int width, const int height, const color bg_color, const color border_color) {
  // Alternative approach: Create a border with a larger rectangle behind a smaller one.
  // This avoids BORDER_STYLE_ROUND_EDGES which may not be supported on older MT5 builds.
  string border_name = name + "_Border";
  int border_thickness = 2;

  // 1. Create the outer rectangle (the border)
  if(ObjectFind(chart_ID, border_name) < 0) {
    ObjectCreate(chart_ID, border_name, OBJ_RECTANGLE_LABEL, 0, 0, 0);
    ObjectSetInteger(chart_ID, border_name, OBJPROP_XDISTANCE, x);
    ObjectSetInteger(chart_ID, border_name, OBJPROP_YDISTANCE, y);
    ObjectSetInteger(chart_ID, border_name, OBJPROP_XSIZE, width);
    ObjectSetInteger(chart_ID, border_name, OBJPROP_YSIZE, height);
    ObjectSetInteger(chart_ID, border_name, OBJPROP_CORNER, CORNER_LEFT_LOWER);
    ObjectSetInteger(chart_ID, border_name, OBJPROP_BGCOLOR, (long)border_color);
    ObjectSetInteger(chart_ID, border_name, OBJPROP_SELECTABLE, false);
    ObjectSetInteger(chart_ID, border_name, OBJPROP_BACK, true);
  }

  // 2. Create the inner rectangle (the background) on top of the border
  CreatePanel(chart_ID, name, x + border_thickness, y + border_thickness, width - (border_thickness * 2), height - (border_thickness * 2), bg_color); // Call the overloaded function
}

void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color) {
  if(ObjectFind(chart_ID, name) < 0) {
    ObjectCreate(chart_ID, name, OBJ_LABEL, 0, 0, 0);
    ObjectSetInteger(chart_ID, name, OBJPROP_XDISTANCE, x);
    ObjectSetInteger(chart_ID, name, OBJPROP_YDISTANCE, y);
    ObjectSetInteger(chart_ID, name, OBJPROP_CORNER, (long)CORNER_LEFT_LOWER);
    ObjectSetInteger(chart_ID, name, OBJPROP_SELECTABLE, false);
    ObjectSetString(chart_ID, name, OBJPROP_FONT, "Arial");
    ObjectSetInteger(chart_ID, name, OBJPROP_FONTSIZE, 10);
    ObjectSetInteger(chart_ID, name, OBJPROP_BACK, false);
  }
  ObjectSetString(chart_ID, name, OBJPROP_TEXT, text);
  ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, text_color);
}

void CreateDashboardRow(long chart_ID, string key, string value, int x_key, int x_val, int y, color clr_key, color clr_val) {
  string key_name = "XauBridge_" + key;
  string val_name = "XauBridge_" + key + "_Val";

  // Create the label for the key (e.g., "RSI")
  CreateLabel(chart_ID, key_name, x_key, y, key, clr_key);
  ObjectSetInteger(chart_ID, key_name, OBJPROP_FONTSIZE, 9);
  ObjectSetString(chart_ID, key_name, OBJPROP_FONT, "Segoe UI");

  // Create the label for the value (e.g., "55.2")
  CreateLabel(chart_ID, val_name, x_val, y, value, clr_val);
  ObjectSetInteger(chart_ID, val_name, OBJPROP_FONTSIZE, 9);
  ObjectSetString(chart_ID, val_name, OBJPROP_FONT, "Segoe UI");
}

void UpdateDashboard(string action, string reason, string rsi, string ema_fast, string ema_slow, string atr, string tp, string sl, double current_spread, double current_balance, double current_pl) {
  long chart_ID = ChartID();
  int x_pos = 15;
  int y_pos = 270; // Increased y_pos to make space for the new P/L row
  int y_step = 17;
  
  color clr_panel_bg = C'33,33,33';
  color clr_panel_border = C'255,193,7'; // Gold accent
  color clr_title = clr_panel_border;
  color clr_label = C'200,200,200'; // Lighter gray for labels
  color clr_value = C'250,250,250'; // White for values
  color clr_spread_ok = C'0,200,83';
  color clr_spread_bad = C'244,67,54';
  color clr_profit = C'0,230,118';
  color clr_loss = C'244,67,54';
  CreatePanel(chart_ID, "XauBridge_Panel", 5, 5, 260, 240, clr_panel_bg, clr_panel_border); // Increased panel height

  string signal_text;
  color signal_color;
  if(action == "buy") {
    signal_text = "ACTION: 📈 BUY";
    signal_color = C'0,230,118';
  } else if(action == "sell") {
    signal_text = "ACTION: 📉 SELL";
    signal_color = C'244,67,54';
  } else {
    signal_text = "ACTION: ⏸️ HOLD";
    signal_color = C'120,144,156';
  }

  CreateLabel(chart_ID, "XauBridge_Title", x_pos, y_pos, "🌉 XAU SCALPER BRIDGE", clr_title);
  ObjectSetString(chart_ID, "XauBridge_Title", OBJPROP_FONT, "Segoe UI Semibold");
  ObjectSetInteger(chart_ID, "XauBridge_Title", OBJPROP_FONTSIZE, 12);
  y_pos -= y_step + 4;
  
  CreateLabel(chart_ID, "XauBridge_Signal", x_pos, y_pos, signal_text, signal_color);
  ObjectSetString(chart_ID, "XauBridge_Signal", OBJPROP_FONT, "Segoe UI");
  ObjectSetInteger(chart_ID, "XauBridge_Signal", OBJPROP_FONTSIZE, 10);
  y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_Reason", x_pos, y_pos, "Reason: " + reason, clr_label);
  ObjectSetString(chart_ID, "XauBridge_Reason", OBJPROP_FONT, "Segoe UI");
  ObjectSetInteger(chart_ID, "XauBridge_Reason", OBJPROP_FONTSIZE, 9);
  y_pos -= y_step + 7;
  
  // --- Two-column layout ---
  int x_val_pos = x_pos + 130;
  
  // Market Info
  CreateDashboardRow(chart_ID, "Balance", DoubleToString(current_balance, 2), x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step;
  CreateDashboardRow(chart_ID, "Spread", DoubleToString(current_spread, 1) + " pts", x_pos, x_val_pos, y_pos, clr_label, (current_spread <= MaxSpreadPoints ? clr_spread_ok : clr_spread_bad)); y_pos -= y_step + 7;
  
  // --- NEW: Current P/L Row ---
  string pl_string = (current_pl == 0.0 && PositionsTotal() == 0) ? "---" : DoubleToString(current_pl, 2);
  color pl_color = (current_pl > 0) ? clr_profit : (current_pl < 0 ? clr_loss : clr_value);
  CreateDashboardRow(chart_ID, "Current P/L", pl_string, x_pos, x_val_pos, y_pos, clr_label, pl_color); y_pos -= y_step + 7;
  
  // Indicators
  CreateDashboardRow(chart_ID, "RSI", rsi, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step;
  CreateDashboardRow(chart_ID, "EMA Fast", ema_fast, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step;
  CreateDashboardRow(chart_ID, "EMA Slow", ema_slow, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step;
  CreateDashboardRow(chart_ID, "ATR", atr, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step + 7;
  
  // Trade Plan
  CreateDashboardRow(chart_ID, "Take Profit Pips", tp, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step;
  CreateDashboardRow(chart_ID, "Stop Loss Pips", sl, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step;
  
  ChartRedraw(chart_ID);
}

// --- Calculation Functions ---

double CalculateLotSize(double stop_loss_pips, string symbol, double point_value) {
  if(stop_loss_pips <= 0) {
    Print("Invalid stop loss (<= 0), cannot calculate lot size.");
    return 0.0;
  }

  double balance = AccountInfoDouble(ACCOUNT_BALANCE);
  double risk_amount = balance * (RiskPercent / 100.0);

  double contract_size = SymbolInfoDouble(symbol, SYMBOL_TRADE_CONTRACT_SIZE);
  double tick_size = SymbolInfoDouble(symbol, SYMBOL_TRADE_TICK_SIZE);
  double tick_value = SymbolInfoDouble(symbol, SYMBOL_TRADE_TICK_VALUE);

  if(contract_size <= 0 || tick_size <= 0 || tick_value <= 0) {
    Print("Invalid symbol properties for lot size calculation.");
    return 0.0;
  }

  double stop_loss_in_price = stop_loss_pips * point_value * 10.0;
  double loss_per_lot = stop_loss_in_price / tick_size * tick_value;

  double lot_size = (loss_per_lot > 0) ? risk_amount / loss_per_lot : 0.0;

  double min_vol = SymbolInfoDouble(symbol, SYMBOL_VOLUME_MIN);
  double max_vol = SymbolInfoDouble(symbol, SYMBOL_VOLUME_MAX);
  double vol_step = SymbolInfoDouble(symbol, SYMBOL_VOLUME_STEP);

  if(vol_step > 0) {
    lot_size = MathRound(lot_size / vol_step) * vol_step;
  }

  if(lot_size < min_vol) lot_size = min_vol;
  if(max_vol > 0 && lot_size > max_vol) lot_size = max_vol;
  
  return lot_size;
}

string GetJsonValue(string json, string key, bool is_string) {
  string search_key;
  string end_char;

  if(is_string) {
    search_key = "\"" + key + "\":\"";
    end_char = "\"";
  } else {
    search_key = "\"" + key + "\":";
    end_char = ",";
  }

  int start_pos = StringFind(json, search_key);
  if(start_pos < 0) return "";
  start_pos += StringLen(search_key);
  int end_pos = StringFind(json, end_char, start_pos);
  if(end_pos < 0) end_pos = StringFind(json, "}", start_pos);
  if(end_pos < 0) return "";
  return StringSubstr(json, start_pos, end_pos - start_pos);
}