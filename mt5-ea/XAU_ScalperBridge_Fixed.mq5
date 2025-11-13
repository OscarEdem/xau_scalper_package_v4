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
input int    SmaPeriod = 200;
input int    StochKPeriod = 14; // New: Stochastic K Period
input int    StochDPeriod = 3;  // New: Stochastic D Period
input int    StochSlowing = 3;  // New: Stochastic Slowing Period
input double SlAtrMultiplier = 1.0;
input double TpAtrMultiplier = 1.5;

// --- Trailing Stop Inputs ---
input bool   UseTrailingStop = true;
input double TrailingStopATRMlt = 1.0; // From best backtest result
input int    TrailingATRPeriod = 14;

// --- Pyramiding Inputs ---
input bool   EnablePyramiding = true;         // Enable adding to winning positions
input int    MaxPyramidEntries = 3;           // Maximum number of simultaneous entries
input double PyramidProfitPips = 15.0;        // Pips in profit required before adding a position
input double PyramidRiskScale = 0.5;          // Scale risk for next entry (e.g., 0.5 = 50% of previous risk)

// --- Logging Inputs ---
input bool   EnableServerLogging = true;

// --- Global Variables ---
CTrade trade;
char post_data[];
char result[];
string result_headers;
int    atr_handle; // Handle for the ATR indicator for trailing stops


// --- NEW: Global variables for backtester-style trailing stop ---
long   g_trade_ticket = 0;       // Ticket of the currently managed trade
double g_high_since_entry = 0.0; // Highest high since the long trade was opened
double g_low_since_entry = 0.0;  // Lowest low since the short trade was opened


// --- Function Prototypes ---
void ClosePositions(ENUM_POSITION_TYPE direction);
void CreatePanel(const long chart_ID, const string name, const int x, const int y, const int width, const int height, const color bg_color, const color border_color);
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color);
void UpdateDashboard(string action, string reason, string rsi, string ema_fast, string ema_slow, string atr, string sma, string stoch_k, string stoch_d, string conviction_score, string tp, string sl, double current_spread, double current_balance, double current_pl);
double CalculateLotSize(double stop_loss_pips, string symbol, double point_value, double risk_percentage_override);
string GetJsonValue(string json, string key, bool is_string);
int CountOpenPositions(ENUM_POSITION_TYPE direction);
void LogEvent(string event_type, ulong ticket, string symbol, string direction, double lot_size, double price, double sl, double tp, double profit, string comment);
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
    UpdateDashboard("hold", "Spread too high", "-", "-", "-", "-", "-", "-", "-", "-", "-", "-", spread_pts, AccountInfoDouble(ACCOUNT_BALANCE), current_pl);
    return;
  }

  // --- Prepare data for server ---
  MqlRates m1_rates[], m5_rates[];
  if(CopyRates(_Symbol, PERIOD_M1, 0, NumCloses, m1_rates) < NumCloses) {
    Print("Could not get enough M1 bar data. Need ", NumCloses, " bars.");
    return;
  }
  if(CopyRates(_Symbol, PERIOD_M5, 0, NumCloses, m5_rates) < NumCloses) {
    Print("Could not get enough M5 bar data. Need ", NumCloses, " bars.");
    return;
  }

  // Build the JSON payload
  string m1_closes_str = "", m1_highs_str = "", m1_lows_str = "";
  string m5_closes_str = "", m5_highs_str = "", m5_lows_str = "";

  for(int i = 0; i < ArraySize(m1_rates); i++) {
    m1_closes_str += DoubleToString(m1_rates[i].close, _Digits);
    m1_highs_str += DoubleToString(m1_rates[i].high, _Digits);
    m1_lows_str += DoubleToString(m1_rates[i].low, _Digits);
    if(i < ArraySize(m1_rates) - 1) {
      m1_closes_str += ",";
      m1_highs_str += ",";
      m1_lows_str += ",";
    }
  }
  
  for(int i = 0; i < ArraySize(m5_rates); i++) {
    m5_closes_str += DoubleToString(m5_rates[i].close, _Digits);
    m5_highs_str += DoubleToString(m5_rates[i].high, _Digits);
    m5_lows_str += DoubleToString(m5_rates[i].low, _Digits);
    if(i < ArraySize(m5_rates) - 1) {
      m5_closes_str += ",";
      m5_highs_str += ",";
      m5_lows_str += ",";
    }
  }

  string json_payload = StringFormat(
    "{\"symbol\":\"%s\",\"timeframe\":\"M1\","
    "\"closes\":[%s],\"highs\":[%s],\"lows\":[%s]," // M1 data
    "\"m5_closes\":[%s],\"m5_highs\":[%s],\"m5_lows\":[%s]," // M5 data
    "\"rsi_period\":%d,\"ema_fast\":%d,\"ema_slow\":%d,\"atr_period\":%d,\"sma_period\":%d," // Existing params
    "\"stoch_k_period\":%d,\"stoch_d_period\":%d,\"stoch_slowing\":%d," // New Stochastic params
    "\"sl_atr_multiplier\":%.1f,\"tp_atr_multiplier\":%.1f}",
    _Symbol,
    m1_closes_str, m1_highs_str, m1_lows_str,
    m5_closes_str, m5_highs_str, m5_lows_str,
    RsiPeriod, EmaFastPeriod, EmaSlowPeriod, AtrPeriod, SmaPeriod, // Existing values
    StochKPeriod, StochDPeriod, StochSlowing, // New Stochastic values
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
  string sma_val_str = "-";
  string stoch_k_val_str = "-"; // New
  string stoch_d_val_str = "-"; // New
  string conviction_score_val_str = "-"; // New

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
    sma_val_str = GetJsonValue(response_str, "sma_last", false);
    stoch_k_val_str = GetJsonValue(response_str, "stoch_k_last", false); // New
    stoch_d_val_str = GetJsonValue(response_str, "stoch_d_last", false); // New
    conviction_score_val_str = GetJsonValue(response_str, "conviction_score", false); // New
    
    PrintFormat("Server response: action=%s, reason=%s, rsi=%s, ema_fast=%s, ema_slow=%s, atr=%s, sma=%s, stoch_k=%s, stoch_d=%s, conviction=%s, tp_pips=%s, sl_pips=%s",
                action, reason, rsi_val, ema_fast, ema_slow, atr_val_str, sma_val_str, stoch_k_val_str, stoch_d_val_str, conviction_score_val_str, tp_pips, sl_pips);
  }

  // --- Display graphical dashboard on chart ---
  UpdateDashboard(action, reason, rsi_val, ema_fast, ema_slow, atr_val_str, sma_val_str, stoch_k_val_str, stoch_d_val_str, conviction_score_val_str, tp_pips, sl_pips, spread_pts, AccountInfoDouble(ACCOUNT_BALANCE), current_pl);

  // --- Trade Execution ---
  double sl = StringToDouble(sl_pips);
  double tp = StringToDouble(tp_pips);
  int conviction = StringToInteger(conviction_score_val_str);

  double adjusted_risk_percent = 0.0;
  if (conviction == 5) {
      adjusted_risk_percent = RiskPercent; // Full risk
  } else if (conviction == 4) {
      adjusted_risk_percent = RiskPercent * 0.75; // 75% risk
  } else if (conviction == 3) {
      adjusted_risk_percent = RiskPercent * 0.5; // 50% risk
  } else {
      // If conviction is too low, do not trade
      adjusted_risk_percent = 0.0;
  }

  /*
    if(action == "buy") {
      ClosePositions(POSITION_TYPE_SELL);
      // Only trade if no open positions and adjusted_risk_percent is positive
      if(PositionsTotal() == 0 && adjusted_risk_percent > 0.0) {
        // Get the high of the current bar for initializing the trailing stop
        MqlRates current_rates[];
        CopyRates(_Symbol, PERIOD_M1, 0, 1, current_rates);
        
        double lot_size = CalculateLotSize(sl, _Symbol, _Point, adjusted_risk_percent);
        double stop_loss_price = bid - sl * _Point * 10.0;
        double take_profit_price = bid + tp * _Point * 10.0;
        if(lot_size > 0 && trade.Buy(lot_size, _Symbol, bid, stop_loss_price, take_profit_price, "XAU Scalper Bridge BUY")) {
          // --- NEW: Initialize trailing stop state on successful trade ---
          g_trade_ticket = trade.ResultDeal();
          // --- NEW: Log the trade opening ---
          string open_details = StringFormat("Conviction: %d; Reason: %s; SL Pips: %.1f; TP Pips: %.1f",
                                             conviction, reason, sl, tp);
          if(PositionSelectByTicket(g_trade_ticket)) {
            LogEvent("Open", g_trade_ticket, _Symbol, "Buy", PositionGetDouble(POSITION_VOLUME), PositionGetDouble(POSITION_PRICE_OPEN),
                     PositionGetDouble(POSITION_SL), PositionGetDouble(POSITION_TP), 0.0, open_details);
          }
  
          g_high_since_entry = current_rates[0].high;
        }
      }
    } else if(action == "sell") {
      ClosePositions(POSITION_TYPE_BUY);
      // Only trade if no open positions and adjusted_risk_percent is positive
      if(PositionsTotal() == 0 && adjusted_risk_percent > 0.0) {
        // Get the low of the current bar for initializing the trailing stop
        MqlRates current_rates[];
        CopyRates(_Symbol, PERIOD_M1, 0, 1, current_rates);
  
        double lot_size = CalculateLotSize(sl, _Symbol, _Point, adjusted_risk_percent);
        double stop_loss_price = ask + sl * _Point * 10.0;
        double take_profit_price = ask - tp * _Point * 10.0;
        if(lot_size > 0 && trade.Sell(lot_size, _Symbol, ask, stop_loss_price, take_profit_price, "XAU Scalper Bridge SELL")) {
          // --- NEW: Initialize trailing stop state on successful trade ---
          g_trade_ticket = trade.ResultDeal();
          // --- NEW: Log the trade opening ---
          string open_details = StringFormat("Conviction: %d; Reason: %s; SL Pips: %.1f; TP Pips: %.1f",
                                             conviction, reason, sl, tp);
          if(PositionSelectByTicket(g_trade_ticket)) {
            LogEvent("Open", g_trade_ticket, _Symbol, "Sell", PositionGetDouble(POSITION_VOLUME), PositionGetDouble(POSITION_PRICE_OPEN),
                     PositionGetDouble(POSITION_SL), PositionGetDouble(POSITION_TP), 0.0, open_details);
          }
  
          g_low_since_entry = current_rates[0].low;
        }
      }
    }
  */
}

void OnTradeTransaction(const MqlTradeTransaction &trans, const MqlTradeRequest &request, const MqlTradeResult &res) {
  // --- NEW: Log trade closures ---
  if(!EnableServerLogging) return;

  // We are interested in completed deals that close a position
  if(trans.type == TRADE_TRANSACTION_DEAL_ADD && trans.deal_type == DEAL_TYPE_BUY || trans.deal_type == DEAL_TYPE_SELL) {
    // A deal is added to history. Check if it closes a position.
    // We can check this by looking for a corresponding position ticket in the deal.
    if(HistoryDealSelect(trans.deal)) {
      long position_id = HistoryDealGetInteger(trans.deal, DEAL_POSITION_ID);
      if(PositionSelectByTicket(position_id)) {
        // Position still exists, this was an entry deal.
        return;
      } else {
        // Position does not exist, this was a closing deal.
        if(HistoryDealGetInteger(trans.deal, DEAL_MAGIC) == MagicNumber) {
          string deal_symbol = HistoryDealGetString(trans.deal, DEAL_SYMBOL);
          if(deal_symbol == _Symbol) {
            ulong ticket = HistoryDealGetInteger(trans.deal, DEAL_TICKET);
            string direction = (HistoryDealGetInteger(trans.deal, DEAL_TYPE) == DEAL_TYPE_BUY) ? "Buy Close" : "Sell Close";
            double lots = HistoryDealGetDouble(trans.deal, DEAL_VOLUME);
            double price = HistoryDealGetDouble(trans.deal, DEAL_PRICE);
            double profit = HistoryDealGetDouble(trans.deal, DEAL_PROFIT);
            string comment = HistoryDealGetString(trans.deal, DEAL_COMMENT);
            
            // Log the closing event
            LogEvent("Close", ticket, deal_symbol, direction, lots, price, 0.0, 0.0, profit, comment);

          }
        }
      }
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

int CountOpenPositions(ENUM_POSITION_TYPE direction) {
  int count = 0;
  for(int i = PositionsTotal() - 1; i >= 0; i--) {
    if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber && PositionGetInteger(POSITION_TYPE) == direction) {
      count++;
    }
  }
  return count;
}

void ManageTrailingStops() {
  // --- Reset tracking variables if our tracked position is closed ---
  if(g_trade_ticket != 0 && PositionSelectByTicket(g_trade_ticket) == false) {
    g_trade_ticket = 0;
    g_high_since_entry = 0.0;
    g_low_since_entry = 0.0;
    return; // No active trade to manage
  }

  // --- Get the high/low of the most recently completed bar ---
  MqlRates rates[1];
  if(CopyRates(_Symbol, PERIOD_M1, 1, 1, rates) < 1) return; // Use bar[1] (last closed bar)
  double last_bar_high = rates[0].high;
  double last_bar_low = rates[0].low;

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
      
      if(PositionGetInteger(POSITION_TYPE) == POSITION_TYPE_BUY) {
        g_high_since_entry = MathMax(g_high_since_entry, last_bar_high);
        new_sl = g_high_since_entry - (current_atr * TrailingStopATRMlt);
        if(new_sl > entry_price && new_sl > current_sl) {
          trade.PositionModify(ticket, new_sl, current_tp);
        }
      } else if(PositionGetInteger(POSITION_TYPE) == POSITION_TYPE_SELL) {
        g_low_since_entry = (g_low_since_entry == 0.0) ? last_bar_low : MathMin(g_low_since_entry, last_bar_low);
        new_sl = g_low_since_entry + (current_atr * TrailingStopATRMlt);
        if(new_sl < entry_price && (current_sl == 0 || new_sl < current_sl)) {
          trade.PositionModify(ticket, new_sl, current_tp);
        }
      }
    }
  }
}

void LogEvent(string event_type, ulong ticket, string symbol, string direction, double lot_size, double price, double sl, double tp, double profit, string comment) {
  if(!EnableServerLogging) return;

  // Escape special characters in comment for JSON
  string json_comment = comment;
  StringReplace(json_comment, "\\", "\\\\");
  StringReplace(json_comment, "\"", "\\\"");

  string log_payload = StringFormat(
    "{\"timestamp\":\"%s\",\"event_type\":\"%s\",\"ticket\":%llu,\"symbol\":\"%s\","
    "\"direction\":\"%s\",\"lot_size\":%.2f,\"price\":%.5f,\"sl\":%.5f,\"tp\":%.5f,"
    "\"profit\":%.2f,\"comment\":\"%s\"}",
    TimeToString(TimeCurrent(), TIME_DATE|TIME_SECONDS), event_type, ticket, symbol,
    direction, lot_size, price, sl, tp, profit, json_comment
  );

  // Use separate buffers for logging to not interfere with the main eval request
  char log_post_data[];
  char log_result[];
  string log_result_headers;
  StringToCharArray(log_payload, log_post_data);
  WebRequest("POST", "http://127.0.0.1:3000/log_trade", "Content-Type: application/json", 1000, log_post_data, log_result, log_result_headers);
}

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

void UpdateDashboard(string action, string reason, string rsi, string ema_fast, string ema_slow, string atr, string sma, string stoch_k, string stoch_d, string conviction_score, string tp, string sl, double current_spread, double current_balance, double current_pl) {
  long chart_ID = ChartID();
  int x_pos = 15;
  // Adjusted y_pos and panel height for new indicators and conviction score
  int y_pos = 321; 
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
  CreatePanel(chart_ID, "XauBridge_Panel", 5, 5, 260, 291, clr_panel_bg, clr_panel_border); // Increased panel height

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
  CreateDashboardRow(chart_ID, "SMA (Trend)", sma, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step;
  CreateDashboardRow(chart_ID, "Stoch %K", stoch_k, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step; // New
  CreateDashboardRow(chart_ID, "Stoch %D", stoch_d, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step + 7; // New
  
  CreateDashboardRow(chart_ID, "Conviction Score", conviction_score, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step + 7; // New
  
  // Trade Plan
  CreateDashboardRow(chart_ID, "Take Profit Pips", tp, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step;
  CreateDashboardRow(chart_ID, "Stop Loss Pips", sl, x_pos, x_val_pos, y_pos, clr_label, clr_value); y_pos -= y_step;
  
  ChartRedraw(chart_ID);
}

// --- Calculation Functions ---

double CalculateLotSize(double stop_loss_pips, string symbol, double point_value, double risk_percentage_override) {
  if(stop_loss_pips <= 0) {
    Print("Invalid stop loss (<= 0), cannot calculate lot size.");
    return 0.0;
  }

  double balance = AccountInfoDouble(ACCOUNT_BALANCE);
  double risk_amount = balance * (risk_percentage_override / 100.0); // Use override

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