// XAU_ScalperBridge.mq5 v4 — production-leaning bridge (Exness).
// Modified to call the external Rust server for trading logic.
#property strict
#property version   "4.2"
#include <Trade\Trade.mqh>

#property description "Calls an external server for XAU/USD scalping signals."

// --- EA Inputs ---
input string ServerUrl = "http://127.0.0.1:3000/eval";
input double RiskPercent = 0.5;
input int    NumCloses = 80;
input double DefaultTP = 5.0;
input double DefaultSL = 10.0;
input double MaxSpreadPoints = 160; // Changed default value to 50
input ulong  MagicNumber = 1337;

// --- Trailing Stop Inputs ---
input bool   UseTrailingStop = true;
input double TrailingStopATRMlt = 1.0; // From best backtest result
input int    TrailingATRPeriod = 14;

// --- Global Variables ---
CTrade trade;
char post_data[];
char result[];
string result_headers;
int    atr_handle; // Handle for the ATR indicator

// --- Function Prototypes ---
void ClosePositions(ENUM_POSITION_TYPE direction);
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color);
void UpdateDashboard(string action, string reason, string rsi, string ema_fast, string ema_slow, double current_spread, double current_balance);
double CalculateLotSize(double stop_loss_pips, string symbol, double point_value);
string GetJsonValue(string json, string key, bool is_string);
void ManageTrailingStops();

int OnInit() {
  // Make sure the terminal is configured to allow WebRequest
  trade.SetExpertMagicNumber(MagicNumber);
  // Go to Tools -> Options -> Expert Advisors and add the ServerUrl
  Print("XAU Scalper Bridge initialized. Server URL: ", ServerUrl);
  return(INIT_SUCCEEDED);
  
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
  Comment(""); // Clear the corner comment
  IndicatorRelease(atr_handle); // Release the indicator handle
  ChartRedraw();
}

void OnTick(){
  static datetime last_bar=0;
  MqlRates rates[];

  // Only run on the open of a new M1 bar
  if(CopyRates(_Symbol, PERIOD_M1, 0, 1, rates) < 1) return;
  if(rates[0].time == last_bar) return;
  last_bar = rates[0].time;

  // --- Pre-trade checks ---
  double ask=SymbolInfoDouble(_Symbol, SYMBOL_ASK);
  double bid=SymbolInfoDouble(_Symbol, SYMBOL_BID);
  double point_val = SymbolInfoDouble(_Symbol, SYMBOL_POINT);
  double spread_raw = ask - bid;
  double spread_pts = spread_raw / point_val;
  
  PrintFormat("Spread Check: Raw Spread=%.5f, Point=%.5f, Spread Points=%.2f", spread_raw, point_val, spread_pts);
  
  if(spread_pts > MaxSpreadPoints) {
    Print("Spread is too high: ", spread_pts, " points. Skipping.");
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

  // The server expects data in chronological order (oldest to newest).
  // CopyRates provides data from oldest [0] to newest [n-1].
  // So we iterate forward to maintain the correct order.
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
    "{\"symbol\":\"%s\",\"timeframe\":\"M1\",\"closes\":[%s],\"highs\":[%s],\"lows\":[%s]}",
    _Symbol,
    closes_str, highs_str, lows_str
  );

  // --- Call the Rust server ---
  ResetLastError();
  StringToCharArray(json_payload, post_data);
  int res = WebRequest("POST", ServerUrl, "Content-Type: application/json", 5000, post_data, result, result_headers);

  if(res == -1) {
    Print("WebRequest failed. Error code: ", GetLastError());
    return;
  }
  
  if(res != 200) {
    Print("Server returned non-200 status: ", res);
    Print("Server response: ", CharArrayToString(result));
    return;
  }

  // --- Process server response ---
  string response_str = CharArrayToString(result);
  // Simple string parsing. For production, a JSON library would be better.
  string action = GetJsonValue(response_str, "action_advice", true);
  string reason = GetJsonValue(response_str, "reason", true);
  string rsi_val = GetJsonValue(response_str, "rsi", false);
  string ema_fast = GetJsonValue(response_str, "ema_fast_last", false);
  string ema_slow = GetJsonValue(response_str, "ema_slow_last", false);
  string tp_pips = GetJsonValue(response_str, "tp_pips", false);
  string sl_pips = GetJsonValue(response_str, "sl_pips", false);
  
  PrintFormat("Server response: action=%s, reason=%s, rsi=%s, ema_fast=%s, ema_slow=%s, tp_pips=%s, sl_pips=%s",
              action, reason, rsi_val, ema_fast, ema_slow, tp_pips, sl_pips);

  // --- Display graphical dashboard on chart ---
  UpdateDashboard(action, reason, rsi_val, ema_fast, ema_slow, spread_pts, AccountInfoDouble(ACCOUNT_BALANCE));

  // --- Trade Execution ---
  double sl = StringToDouble(sl_pips);
  double tp = StringToDouble(tp_pips);

  if(action == "buy") {
    // Close any open sell positions before opening a new buy
    ClosePositions(POSITION_TYPE_SELL);
    
    // Open a new buy trade if none exist
    if(PositionsTotal() == 0) {
      double lot_size = CalculateLotSize(sl, _Symbol, _Point);
      double stop_loss_price = bid - sl * _Point * 10; // SL is in pips, 1 pip = 10 points for XAUUSD
      double take_profit_price = bid + tp * _Point * 10; // TP is in pips
      trade.Buy(lot_size, _Symbol, bid, stop_loss_price, take_profit_price, "XAU Scalper Bridge BUY");
    }

  } else if(action == "sell") {
    // Close any open buy positions before opening a new sell
    ClosePositions(POSITION_TYPE_BUY);

    // Open a new sell trade if none exist
    if(PositionsTotal() == 0) {
      double lot_size = CalculateLotSize(sl, _Symbol, _Point);
      double stop_loss_price = ask + sl * _Point * 10; // SL is in pips
      double take_profit_price = ask - tp * _Point * 10; // TP is in pips
      trade.Sell(lot_size, _Symbol, ask, stop_loss_price, take_profit_price, "XAU Scalper Bridge SELL");
    }
  }
  
  // --- Trailing Stop Management (runs on every tick) ---
  // This part is outside the new bar check
  if(UseTrailingStop) {
    ManageTrailingStops();
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
  // Get the latest ATR value
  double atr_buffer[];
  if(CopyBuffer(atr_handle, 0, 0, 1, atr_buffer) < 1) {
    Print("Could not get ATR value for trailing stop.");
    return;
  }
  double current_atr = atr_buffer[0];

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
        // Calculate potential new stop loss
        new_sl = current_bid - (current_atr * TrailingStopATRMlt);
        
        // Check if the new SL is higher than the entry price and also higher than the current SL
        if(new_sl > entry_price && new_sl > current_sl) {
          // Modify the position with the new trailing stop
          if(!trade.PositionModify(ticket, new_sl, current_tp)) {
            Print("Error modifying position #", ticket, " for trailing stop. Code: ", GetLastError());
          }
        }
      }
      
      // --- Logic for a SHORT position ---
      else if(PositionGetInteger(POSITION_TYPE) == POSITION_TYPE_SELL) {
        double current_ask = SymbolInfoDouble(_Symbol, SYMBOL_ASK);
        // Calculate potential new stop loss
        new_sl = current_ask + (current_atr * TrailingStopATRMlt);
        
        // Check if the new SL is lower than the entry price and also lower than the current SL
        if(new_sl < entry_price && (new_sl < current_sl || current_sl == 0)) {
          // Modify the position with the new trailing stop
          if(!trade.PositionModify(ticket, new_sl, current_tp)) {
            Print("Error modifying position #", ticket, " for trailing stop. Code: ", GetLastError());
          }
        }
      }
    }
  }
}
// --- Graphical Dashboard Functions ---

// Helper to create or update a text label on the chart
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color) {
  // Create the object if it doesn't exist
  if(ObjectFind(chart_ID, name) != 0) {
    ObjectCreate(chart_ID, name, OBJ_LABEL, 0, 0, 0);
    ObjectSetInteger(chart_ID, name, OBJPROP_XDISTANCE, x);
    ObjectSetInteger(chart_ID, name, OBJPROP_YDISTANCE, y);
    ObjectSetInteger(chart_ID, name, OBJPROP_CORNER, CORNER_LEFT_LOWER);
    ObjectSetInteger(chart_ID, name, OBJPROP_SELECTABLE, false);
    ObjectSetString(chart_ID, name, OBJPROP_FONT, "Calibri");
    ObjectSetInteger(chart_ID, name, OBJPROP_FONTSIZE, 10);
  }
  // Update text and color
  ObjectSetString(chart_ID, name, OBJPROP_TEXT, text);
  ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, text_color);
}

// Main function to draw the entire dashboard
void UpdateDashboard(string action, string reason, string rsi, string ema_fast, string ema_slow, double current_spread, double current_balance) {
  long chart_ID = ChartID();
  int x_pos = 10;
  int y_pos = 167; // Adjusted for more lines
  int y_step = 16;
  color default_color = clrGold;

  // Determine signal text and color
  string signal_text;
  color signal_color;
  if(action == "buy") {
    signal_text = "Signal : 📈 BUY";
    signal_color = clrLimeGreen;
  } else if(action == "sell") {
    signal_text = "Signal : 📉 SELL";
    signal_color = clrRed;
  } else {
    signal_text = "Signal : ⏸️ HOLD";
    signal_color = clrSilver;
  }

  CreateLabel(chart_ID, "XauBridge_Title", x_pos, y_pos, "🌉 XAU SCALPER BRIDGE v4.2", default_color); y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_Sep1", x_pos, y_pos, "━━━━━━━━━━━━━━━━━━━━━━━━━━", default_color); y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_Signal", x_pos, y_pos, signal_text, signal_color); y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_Reason", x_pos, y_pos, "Reason : " + reason, default_color); y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_Spread", x_pos, y_pos, "Spread   : " + DoubleToString(current_spread, 1) + " pts", default_color); y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_Balance", x_pos, y_pos, "Balance  : " + DoubleToString(current_balance, 2), default_color); y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_Sep2", x_pos, y_pos, "──────────────────────────", default_color); y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_RSI", x_pos, y_pos, "RSI      : " + rsi, default_color); y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_EMAFast", x_pos, y_pos, "EMA Fast : " + ema_fast, default_color); y_pos -= y_step;
  CreateLabel(chart_ID, "XauBridge_EMASlow", x_pos, y_pos, "EMA Slow : " + ema_slow, default_color); y_pos -= y_step;
  
  ChartRedraw(chart_ID);
}

// --- Calculation Functions ---

double CalculateLotSize(double stop_loss_pips, string symbol, double point_value) {
  if(stop_loss_pips <= 0) {
    Print("Invalid stop loss (<= 0), cannot calculate lot size.");
    return 0.0; // Return 0 to prevent trading
  }

  double balance = AccountInfoDouble(ACCOUNT_BALANCE);
  double risk_amount = balance * (RiskPercent / 100.0);

  // Get symbol properties for lot size calculation
  double contract_size = SymbolInfoDouble(symbol, SYMBOL_TRADE_CONTRACT_SIZE);
  double tick_size = SymbolInfoDouble(symbol, SYMBOL_TRADE_TICK_SIZE);
  double tick_value = SymbolInfoDouble(symbol, SYMBOL_TRADE_TICK_VALUE);

  if(contract_size <= 0 || tick_size <= 0 || tick_value <= 0) {
    Print("Invalid symbol properties for lot size calculation.");
    return 0.0;
  }

  // For XAUUSD, 1 pip = 0.1 price move. The server sends SL in pips.
  double stop_loss_in_price = stop_loss_pips * point_value * 10;
  double loss_per_lot = stop_loss_in_price * (tick_value / tick_size);

  double lot_size = (loss_per_lot > 0) ? risk_amount / loss_per_lot : 0.0;

  // Normalize and check against volume limits
  double min_vol = SymbolInfoDouble(symbol, SYMBOL_VOLUME_MIN);
  double max_vol = SymbolInfoDouble(symbol, SYMBOL_VOLUME_MAX);
  double vol_step = SymbolInfoDouble(symbol, SYMBOL_VOLUME_STEP);

  if(vol_step > 0)
  {
    lot_size = MathRound(lot_size / vol_step) * vol_step;
  }

  if(lot_size < min_vol) lot_size = min_vol;
  if(max_vol > 0 && lot_size > max_vol) lot_size = max_vol;
  
  return lot_size;
}

// A very basic helper to extract a value from a JSON string.
// is_string: set to true if the expected value is a string in quotes, false for numbers.
string GetJsonValue(string json, string key, bool is_string) {
  string search_key;
  string end_char;

  if(is_string) {
    search_key = "\"" + key + "\":\"";
    end_char = "\"";
  } else {
    search_key = "\"" + key + "\":";
    end_char = ","; // Numbers are usually followed by a comma or a closing brace
  }

  int start_pos = StringFind(json, search_key);
  if(start_pos < 0) return "";
  start_pos += StringLen(search_key);
  int end_pos = StringFind(json, end_char, start_pos);
  // If the end character isn't found (e.g., it's the last field), look for the closing brace
  if(end_pos < 0) end_pos = StringFind(json, "}", start_pos);
  if(end_pos < 0) return "";
  return StringSubstr(json, start_pos, end_pos - start_pos);
}
