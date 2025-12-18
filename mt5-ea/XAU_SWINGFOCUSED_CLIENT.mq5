// XAU_SWINGFOCUSED_CLIENT.mq5
// Client-side Signal Receiver & Executor
// DOES NOT send market data. Only fetches signals and executes trades.
#property strict
#property version   "1.0"
#include <Trade\Trade.mqh>

// --- EA Inputs ---
input string ServerUrl = "https://gold-ml-base-server.onrender.com"; // Base URL/
input int    UpdateIntervalSeconds = 1;       // How often to check for signals (seconds)
input int    SafetyTimeoutSeconds = 30;       // Circuit Breaker Trigger Time
input double RiskPercent = 1.0;               // Risk per trade (% of Balance)
input double MaxSpreadPoints = 162;           // Max local spread allowed to take trade
input double MinRiskReward = 0.0;             // Minimum R:R ratio to accept trade
input ulong  MagicNumber = 1337;
input bool   ScalpMode = true;                // Enable Scalping Mode (No Pyramid, Hard Time Stop)
input int    MaxTradeLifetimeSeconds = 180;    // Max seconds to hold a scalp trade
input double MinVelocityPips = 0.5;           // Min volatility required (Pips)
input int    VelocityLookbackSecs = 10;       // Seconds to measure volatility
input double MaxSpreadAtrRatio = 0.15;        // Max Spread/ATR ratio
input double SlTightenPips     = 100.0;       // Pips to tighten Server SL (make closer to entry)

input double FixedLotSize      = 0.01;        // Static Lot Size per entry
input int    MaxSignalEntries  = 3;           // Max entries based on conviction
// --- Visual Settings ---
input color  ColorLimitBuy     = C'0,191,255'; // Limit Buy Arrow Color (DeepSkyBlue)
input color  ColorLimitSell    = C'255,140,0'; // Limit Sell Arrow Color (DarkOrange)
input color  ColorMarketBuy    = C'46,204,113';// Market Buy Arrow Color (Emerald)
input color  ColorMarketSell   = C'231,76,60'; // Market Sell Arrow Color (Alizarin)

// --- Pyramiding Inputs ---
input bool   EnablePyramiding = true;         // Enable adding to winning positions
input int    MaxPyramidEntries = 3;           // Maximum number of simultaneous entries
input double PyramidProfitPips = 10.0;        // Pips in profit required before adding
input double DashboardScale = 0.0;            // Scale factor (0.0 = Auto-scale)

// --- Trailing SL Inputs ---
input bool   UseTrailingSL = true;            // Enable Trailing Stop
input bool   UseAtrTrailing = false;          // Use ATR for Trailing calculations
input double AtrMultStart  = 1.5;             // ATR Multiplier for Start
input double AtrMultDist   = 1.0;             // ATR Multiplier for Distance
input double AtrMultStep   = 0.1;             // ATR Multiplier for Step
input double TrailingStartPips = 20.0;        // Pips profit to start trailing
input double TrailingDistPips  = 70;        // Distance from price
input double TrailingStepPips  = 2.0;         // Minimum step to modify SL
input double BreakEvenTriggerPips = 15.0;     // Pips profit to move to BE
input double BreakEvenOffsetPips  = 1.0;      // Pips to lock in at BE
input bool   UseStagnationSL   = false;       // Tighten SL if trade stalls
input int    StagnationSeconds = 60;         // Seconds before tightening SL (1 min)
input double StagnationRiskReducePct = 20.0;  // % of risk to remove (0-100)
input bool   UseDynamicStagnation = true;     // Scale stagnation time by ATR
input double StagnationTimeMult   = 2.0;      // Multiplier for ATR-based time
input int    LimitOrderExpirationSeconds = 240; // Auto-delete pending orders after X seconds
input bool   CloseOnDisconnect = false;       // If true, closes all trades on server timeout

// --- THEME & SIZING HELPERS ---
color THEME_BG      = C'0,0,0';       // Solid Black
color THEME_BORDER  = C'212,175,55';  // Gold accent
color THEME_TEXT    = C'230,230,230';
color THEME_ACCENT  = C'46,204,113';  // Buy (Emerald Green)
color THEME_ACCENT2 = C'231,76,60';   // Sell (Alizarin Red)
color THEME_MUTED   = C'140,140,140';
#define FONT_FACE     "Segoe UI"

// --- Global Structures ---
struct ServerSignal
  {
   string            action;
   string            entryType;
   string            reason;
   double            slPrice;
   double            tp1Price;
   double            limitPrice;
   string            recommendedOrderType;
   double            conviction;
   long              expiration;
   ulong             ticketToManage;
   double            newSlPrice;
   string            fundamentalBias;
   string            rrStr;
   bool              isNew;
   string            jsonResponse; // Store raw JSON for drawing zones
  };

// --- Global Variables ---
CTrade trade;
char result[];
bool g_ShowZones = true;
bool g_ShowLiq = true;
bool g_ShowFVG = true;
bool g_ShowOB = true;
bool g_ShowArrows = true;
bool g_compactMode = false;
bool g_ForceMarketOrders = false; // New toggle for limit->market conversion
int g_DashboardX = -1;
int g_DashboardY = -1;
bool g_IsDragging = false;
int g_DragOffsetX = 0;
int g_DragOffsetY = 0;
double g_ServerATR = 0.0; // ATR received from server
double g_UserLotSize = 0.01;
int    g_UserMaxEntries = 3;
bool   g_UserScalpMode = true;
bool   g_UserTrailingSL = true;
int    g_UserStagnationSeconds = 60;
bool   g_ShowSettings = false;
ServerSignal g_SignalState;
datetime g_LastHeartbeat = 0;
bool g_CircuitBreakerTripped = false;
bool g_IsRequestPending = false;

// --- Dashboard State Globals ---
string g_DashAction = "hold";
string g_DashReason = "Waiting for Server...";
string g_DashTP = "-";
string g_DashSL = "-";
double g_DashSpread = 0.0;
double g_DashBalance = 0.0;
double g_DashPL = 0.0;
double g_DashLot = 0.0;
double g_DashConviction = 0.0;
string g_DashBias = "Neutral";
string g_DashRR = "-";
string g_DashEntryType = "none";
string g_DashStagnationCountdown = "-";

// --- Function Prototypes ---
void ClosePositions(ENUM_POSITION_TYPE direction);
void UpdateDashboard(string action, string reason, string tp, string sl, double current_spread, double current_balance, double current_pl, double next_lot_size, double conviction, string bias, string rr);
void DrawDashboard();
double GetUiScale();
int ui(int px);
void DrawConvictionBar(long chart_ID, int x, int y, int totalWidthPx, int heightPx, double convictionPercent);
int CalculateEntryCount(int conviction);
string GetJsonValue(string json, string key, bool is_string);
string GetNestedJsonValue(string json, string object_key, string value_key, bool is_string);
string GetJsonObject(string json, string key);
int CountOpenPositions(ENUM_POSITION_TYPE direction);
int CountPendingOrders(ENUM_ORDER_TYPE type);
void CreateSignalArrow(long chart_ID, string name, datetime time, double price, int arrow_code, color clr);
void DrawServerLiquidityZones(long chart_ID, string json_response);
void DrawServerImbalances(long chart_ID, string json_response);
void DrawServerOrderBlocks(long chart_ID, string json_response);
void CreateChartLabel(long chart_ID, string name, datetime time, double price, string text, color clr, int anchor);
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color, int font_size=10, string font=FONT_FACE);
void CreateDashboardRow(long chart_ID, string key, string value, int x_key, int x_val, int y, color clr_key, color clr_val);
void CreateControlRow(long chart_ID, string key, string value, int x, int y, string btnMinus, string btnPlus);
void CreateToggleRow(long chart_ID, string key, bool value, int x, int y, string btnName);
void DrawSettings(long chart_ID, int x, int y, int w, int h);
void CreateToggleButton(long chart_ID, int x, int y);
void CreateForceMarketButton(long chart_ID, int x, int y);
void UpdateForceMarketButtonState();
void CreateArrowToggleButton(long chart_ID, int x, int y);
void CreateCollapseButton(long chart_ID, int x, int y, bool is_collapsed);
void UpdateButtonState();
void UpdateArrowButtonState();
void CreateCloseAllButton(long chart_ID, int x, int y);
void CreateConfigButton(long chart_ID, int x, int y);
void CloseAllTrades();
void FetchServerData();
void CheckConnectivity();
void ProcessSignal();
void ManageTrailingSL();
void ManagePendingOrders();

//+------------------------------------------------------------------+
//| Initialization                                                   |
//+------------------------------------------------------------------+
int OnInit()
  {
   trade.SetExpertMagicNumber(MagicNumber);
   ChartSetInteger(0, CHART_EVENT_MOUSE_MOVE, true);
   Print("XAU Client Bridge initialized. Listening to: ", ServerUrl);
   g_UserLotSize = FixedLotSize;
   g_UserMaxEntries = MaxSignalEntries;
   g_UserScalpMode = ScalpMode;
   g_UserTrailingSL = UseTrailingSL;
   g_UserStagnationSeconds = StagnationSeconds;

// Initialize State
   g_SignalState.action = "hold";
   g_SignalState.isNew = false;
   g_LastHeartbeat = TimeCurrent();

   EventSetTimer(UpdateIntervalSeconds);
   UpdateDashboard("hold", "Connecting...", "-", "-", 0.0, AccountInfoDouble(ACCOUNT_BALANCE), 0.0, 0.0, 0.0, "Neutral", "-");
   return(INIT_SUCCEEDED);
  }

//+------------------------------------------------------------------+
//| Deinitialization                                                 |
//+------------------------------------------------------------------+
void OnDeinit(const int reason)
  {
   EventKillTimer();
   ObjectsDeleteAll(0, "XauBridge_");
   ObjectsDeleteAll(0, "XauBridge_Arrow_");
   ObjectsDeleteAll(0, "XauBridge_SrvOB_");
   ChartRedraw();
  }

//+------------------------------------------------------------------+
//| Timer Event (The Networking Loop)                                |
//+------------------------------------------------------------------+
void OnTimer()
  {
// Prevent re-entry if the previous request is somehow stuck
   if(g_IsRequestPending)
      return;

   g_IsRequestPending = true;
   FetchServerData();
   g_IsRequestPending = false;
  }

//+------------------------------------------------------------------+
//| Main Loop                                                        |
//+------------------------------------------------------------------+
void OnTick()
  {
// 1. Check Circuit Breaker (CRITICAL SAFETY)
   CheckConnectivity();

   if(g_CircuitBreakerTripped)
     {
      // In emergency mode, we ONLY manage existing stops or close out.
      if(CloseOnDisconnect)
         CloseAllTrades();
      else
         ManageTrailingSL(); // Fallback to local trailing

      // Update Dashboard to show disconnected state
      UpdateDashboard("hold", "DISCONNECTED", "-", "-", 0.0, AccountInfoDouble(ACCOUNT_BALANCE), 0.0, 0.0, 0.0, "Safety Mode", "-");
      return;
     }

// Manage Trailing SL
   ManageTrailingSL();
   ManagePendingOrders();

// 3. Process New Signals
   if(g_SignalState.isNew)
     {
      ProcessSignal();
      g_SignalState.isNew = false; // Mark as handled
     }
   else
     {
      // Refresh dashboard P/L and Spread even if no new signal
      double current_pl = 0.0;
      for(int i = PositionsTotal() - 1; i >= 0; i--)
        {
         if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber)
            current_pl += PositionGetDouble(POSITION_PROFIT);
        }

      double ask = SymbolInfoDouble(_Symbol, SYMBOL_ASK);
      double bid = SymbolInfoDouble(_Symbol, SYMBOL_BID);
      double spread_pts = (ask - bid) / SymbolInfoDouble(_Symbol, SYMBOL_POINT);

      UpdateDashboard(g_SignalState.entryType, g_SignalState.reason,
                      g_SignalState.tp1Price > 0 ? DoubleToString(g_SignalState.tp1Price, _Digits) : "-",
                      g_SignalState.slPrice > 0 ? DoubleToString(g_SignalState.slPrice, _Digits) : "-",
                      spread_pts, AccountInfoDouble(ACCOUNT_BALANCE), current_pl,
                      0.0, g_SignalState.conviction, g_SignalState.fundamentalBias, g_SignalState.rrStr);
     }
  }

//+------------------------------------------------------------------+
//| Network Function                                                 |
//+------------------------------------------------------------------+
void FetchServerData()
  {
   char post_data[];
   char get_result[];
   string get_headers;
   string url = ServerUrl + "/signals/" + _Symbol;

   ResetLastError();
// TIMEOUT REDUCED to 2000ms. If server is slow, we skip this beat rather than hanging.
   int res = WebRequest("GET", url, NULL, 2000, post_data, get_result, get_headers);

   if(res == 200)
     {
      string response_str = CharArrayToString(get_result);

      // Update Heartbeat
      g_LastHeartbeat = TimeCurrent();

      // Parse Logic
      g_SignalState.jsonResponse = GetJsonObject(response_str, "swingSignal");
      g_SignalState.entryType = GetNestedJsonValue(response_str, "scalpSignal", "entryType", true);
      g_SignalState.reason = GetNestedJsonValue(response_str, "scalpSignal", "reason", true);
      g_SignalState.slPrice = StringToDouble(GetNestedJsonValue(response_str, "scalpSignal", "slPrice", false));
      g_SignalState.tp1Price = StringToDouble(GetNestedJsonValue(response_str, "scalpSignal", "tp1Price", false));
      g_SignalState.limitPrice = StringToDouble(GetNestedJsonValue(response_str, "scalpSignal", "limitOrderPrice", false));
      g_SignalState.recommendedOrderType = GetNestedJsonValue(response_str, "scalpSignal", "recommendedOrderType", true);
      g_SignalState.expiration = (long)StringToInteger(GetNestedJsonValue(response_str, "scalpSignal", "expirationSeconds", false));
      g_SignalState.conviction = StringToDouble(GetNestedJsonValue(response_str, "scalpSignal", "convictionScore", false));
      g_SignalState.fundamentalBias = GetJsonValue(response_str, "bias", true);

      g_SignalState.action = GetNestedJsonValue(response_str, "swingSignal", "action", true);
      g_SignalState.ticketToManage = (ulong)StringToInteger(GetNestedJsonValue(response_str, "swingSignal", "ticketToManage", false));
      g_SignalState.newSlPrice = StringToDouble(GetNestedJsonValue(response_str, "swingSignal", "slPrice", false));

      // Parse ATR from Server Debug Info
      string scalp_json = GetJsonObject(response_str, "scalpSignal");
      string debug_json = GetJsonObject(scalp_json, "debugInfo");
      string atr_str = GetJsonValue(debug_json, "atr", true);
      if(atr_str != "")
         g_ServerATR = StringToDouble(atr_str);

      // --- Adjust SL Closer to Entry ---
      double point_val = SymbolInfoDouble(_Symbol, SYMBOL_POINT);
      double ask = SymbolInfoDouble(_Symbol, SYMBOL_ASK);
      double bid = SymbolInfoDouble(_Symbol, SYMBOL_BID);

      if(g_SignalState.slPrice > 0.0 && SlTightenPips != 0.0)
        {
         double adj = SlTightenPips * point_val * 10.0;
         if(StringFind(g_SignalState.entryType, "long") >= 0)
           {
            g_SignalState.slPrice += adj; // Move SL Up
            if(g_SignalState.slPrice >= bid)
               g_SignalState.slPrice = bid - (20.0 * point_val);
           }
         else
            if(StringFind(g_SignalState.entryType, "short") >= 0)
              {
               g_SignalState.slPrice -= adj; // Move SL Down
               if(g_SignalState.slPrice <= ask)
                  g_SignalState.slPrice = ask + (20.0 * point_val);
              }
        }

      g_SignalState.isNew = true;
     }
   else
     {
      Print("Network Error: ", res, " Last Error: ", GetLastError());
      // We do NOT update g_LastHeartbeat here.
      // This will eventually trip the circuit breaker in OnTick.
     }
  }

//+------------------------------------------------------------------+
//| Circuit Breaker Logic                                            |
//+------------------------------------------------------------------+
void CheckConnectivity()
  {
   long time_since_update = TimeCurrent() - g_LastHeartbeat;

   if(time_since_update > SafetyTimeoutSeconds)
     {
      if(!g_CircuitBreakerTripped)
        {
         Print("CRITICAL: Connection Lost! Last heartbeat: ", time_since_update, "s ago. Engaged Safety Mode.");
         g_CircuitBreakerTripped = true;
        }
     }
   else
     {
      if(g_CircuitBreakerTripped)
        {
         Print("Connection Restored. Resuming normal operation.");
         g_CircuitBreakerTripped = false;
        }
     }
  }

//+------------------------------------------------------------------+
//| Signal Processing                                                |
//+------------------------------------------------------------------+
void ProcessSignal()
  {
   double ask = SymbolInfoDouble(_Symbol, SYMBOL_ASK);
   double bid = SymbolInfoDouble(_Symbol, SYMBOL_BID);
   double point_val = SymbolInfoDouble(_Symbol, SYMBOL_POINT);
   double spread_raw = ask - bid;
   double spread_pts = spread_raw / point_val;

// Unpack state for easier usage
   string action = g_SignalState.action;
   string reason = g_SignalState.reason;
   string entry_type = g_SignalState.entryType;
   string tp_pips = "-";
   string sl_pips = "-";
   double sl_price = g_SignalState.slPrice;
   double limit_price = g_SignalState.limitPrice;
   double tp1_price = g_SignalState.tp1Price;
   string recommended_order_type = g_SignalState.recommendedOrderType;
   long expiration_seconds = g_SignalState.expiration;
   ulong ticket_to_manage = g_SignalState.ticketToManage;
   double new_sl_price = g_SignalState.newSlPrice;
   double conviction_score = g_SignalState.conviction;
   string fundamental_bias = g_SignalState.fundamentalBias;
   string rr_str = "-";

// Calculate Pips for Display
   if(StringFind(entry_type, "long") >= 0 && sl_price > 0)
     {
      sl_pips = DoubleToString((ask - sl_price) / (point_val * 10.0), 1);
      tp_pips = DoubleToString((tp1_price - ask) / (point_val * 10.0), 1);
      double r = MathAbs(ask - sl_price);
      double rw = MathAbs(tp1_price - ask);
      if(r > 0)
         rr_str = DoubleToString(rw/r, 2) + "R";
     }
   else
      if(StringFind(entry_type, "short") >= 0 && sl_price > 0)
        {
         sl_pips = DoubleToString((sl_price - bid) / (point_val * 10.0), 1);
         tp_pips = DoubleToString((bid - tp1_price) / (point_val * 10.0), 1);
         double r = MathAbs(sl_price - bid);
         double rw = MathAbs(bid - tp1_price);
         if(r > 0)
            rr_str = DoubleToString(rw/r, 2) + "R";
        }

// Update Visuals
   if(g_ShowZones)
     {
      if(g_ShowFVG) DrawServerImbalances(0, g_SignalState.jsonResponse);
      if(g_ShowLiq) DrawServerLiquidityZones(0, g_SignalState.jsonResponse);
      if(g_ShowOB) DrawServerOrderBlocks(0, g_SignalState.jsonResponse);
     }

// Draw Arrows on Signal Change (Simple check based on time/existence)
   MqlRates rates[];
   if(CopyRates(_Symbol, PERIOD_M1, 0, 1, rates) > 0)
     {
      string arrow_name = "XauBridge_Arrow_" + TimeToString(rates[0].time);
      double ref_price_long = ask;
      double ref_price_short = bid;
      color arrow_color_long = ColorMarketBuy;
      color arrow_color_short = ColorMarketSell;

      if(recommended_order_type == "limit_long" && limit_price > 0)
        {
         ref_price_long = limit_price;
         arrow_color_long = ColorLimitBuy;
        }
      if(recommended_order_type == "limit_short" && limit_price > 0)
        {
         ref_price_short = limit_price;
         arrow_color_short = ColorLimitSell;
        }

      if(g_ShowArrows)
        {
         // Check for existing arrow and update if direction changed (e.g. Short -> Long flip on same bar)
         if(ObjectFind(0, arrow_name) >= 0)
           {
            long existing_code = ObjectGetInteger(0, arrow_name, OBJPROP_ARROWCODE);
            // If signal is long but existing arrow is short (234), delete it
            if(StringFind(entry_type, "long") >= 0 && existing_code == 234)
               ObjectDelete(0, arrow_name);
            // If signal is short but existing arrow is long (233), delete it
            else
               if(StringFind(entry_type, "short") >= 0 && existing_code == 233)
                  ObjectDelete(0, arrow_name);
           }

         if(StringFind(entry_type, "long") >= 0 && ObjectFind(0, arrow_name) < 0)
            CreateSignalArrow(0, arrow_name, rates[0].time, ref_price_long - 100*point_val, 233, arrow_color_long);
         else
            if(StringFind(entry_type, "short") >= 0 && ObjectFind(0, arrow_name) < 0)
               CreateSignalArrow(0, arrow_name, rates[0].time, ref_price_short + 100*point_val, 234, arrow_color_short);
        }
     }

// 3. Trade Management (SL Update / Close)
   if(action == "close" && ticket_to_manage > 0)
     {
      if(trade.PositionClose(ticket_to_manage))
         Print("Server requested Close on ticket ", ticket_to_manage);
     }
   else
      if(action == "update_sl" && ticket_to_manage > 0 && new_sl_price > 0)
        {
         if(PositionSelectByTicket(ticket_to_manage))
            trade.PositionModify(ticket_to_manage, new_sl_price, PositionGetDouble(POSITION_TP));
        }

// 4. Entry Logic
   double entry_price = 0.0;
   if(StringFind(entry_type, "long") >= 0)
      entry_price = ask;
   else
      if(StringFind(entry_type, "short") >= 0)
         entry_price = bid;

// Calculate Entry Count based on Conviction
   int entry_count = CalculateEntryCount((int)conviction_score);

// SAFETY: If entry type is not explicitly long or short, or is blocked, force count to 0
   if(entry_price == 0.0 || entry_type == "none" || StringFind(entry_type, "blocked") >= 0)
      entry_count = 0;

// Check Velocity (New)
   if(entry_count > 0)
     {
      MqlTick ticks[];
      ulong from_msc = (ulong)(TimeCurrent() - VelocityLookbackSecs) * 1000;
      int copied = CopyTicksRange(_Symbol, ticks, COPY_TICKS_INFO, from_msc, 0);
      double vel_range = 0.0;
      if(copied > 0)
        {
         double min_p = DBL_MAX;
         double max_p = 0.0;
         for(int i=0; i<copied; i++)
           {
            if(ticks[i].bid < min_p)
               min_p = ticks[i].bid;
            if(ticks[i].bid > max_p)
               max_p = ticks[i].bid;
           }
         vel_range = (max_p - min_p) / (point_val * 10.0);
        }

      if(vel_range < MinVelocityPips)
        {
         reason = "Low Volatility (" + DoubleToString(vel_range, 1) + ")";
         entry_count = 0;
        }
     }

// Check Spread limit locally (Dynamic)
   bool spread_fail = false;
   if(g_ServerATR > 0)
     {
      if(spread_raw > g_ServerATR * MaxSpreadAtrRatio)
         spread_fail = true;
     }
   else
     {
      if(spread_pts > MaxSpreadPoints)
         spread_fail = true;
     }

   if(spread_fail && entry_count > 0)
     {
      reason = "Spread High (" + DoubleToString(spread_pts,1) + ")";
      entry_count = 0; // Block trade
     }

// Check R:R limit
   if(MinRiskReward > 0.0 && entry_count > 0)
     {
      double risk = MathAbs(entry_price - sl_price);
      double reward = MathAbs(tp1_price - entry_price);
      if(risk > 0 && (reward / risk) < MinRiskReward)
        {
         if(!g_ForceMarketOrders)
           {
            reason = "Low R:R (" + DoubleToString(reward/risk, 2) + ")";
            entry_count = 0;
           }
        }
     }

// Calculate P/L
   double current_pl = 0.0;
   for(int i = PositionsTotal() - 1; i >= 0; i--)
     {
      if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber)
         current_pl += PositionGetDouble(POSITION_PROFIT);
     }

// Update global state strings for dashboard (which is drawn in OnTick)
   g_SignalState.rrStr = rr_str;
// Note: Dashboard drawing happens in OnTick using these values
   UpdateDashboard(entry_type, reason, tp_pips, sl_pips, spread_pts, AccountInfoDouble(ACCOUNT_BALANCE), current_pl, (double)entry_count, conviction_score, fundamental_bias, rr_str);

// Execution
   if(entry_count > 0 && action != "close" && action != "update_sl")
     {
      if(recommended_order_type == "limit_long" || recommended_order_type == "market_long")
        {
         ClosePositions(POSITION_TYPE_SELL); // Hedge/Flip
         int open_buys = CountOpenPositions(POSITION_TYPE_BUY);
         int pending_buys = CountPendingOrders(ORDER_TYPE_BUY_LIMIT);

         // If forcing market, delete pending orders to allow execution
         if(g_ForceMarketOrders && pending_buys > 0)
           {
            Print("Force Market ON: Deleting pending buy limits to execute market order.");
            for(int i=OrdersTotal()-1; i>=0; i--)
               if(OrderGetString(ORDER_SYMBOL)==_Symbol && OrderGetInteger(ORDER_MAGIC)==MagicNumber && OrderGetInteger(ORDER_TYPE)==ORDER_TYPE_BUY_LIMIT)
                  trade.OrderDelete(OrderGetTicket(i));
            pending_buys = 0;
           }

         // Execute if under max entries, regardless of P/L (allows averaging/independent signals)
         if(pending_buys == 0 && open_buys < g_UserMaxEntries)
           {
            // Check if Limit price is better than market (Buy Limit >= Ask), convert to Market
            bool force_market = (recommended_order_type == "limit_long" && limit_price > 0 && limit_price >= ask) || g_ForceMarketOrders;
            if(force_market && !g_ForceMarketOrders)
               Print("Limit Buy Price ", limit_price, " >= Ask ", ask, ". Converting to Market Buy.");

            if(recommended_order_type == "limit_long" && limit_price > 0 && !force_market)
              {
               datetime exp = (expiration_seconds > 0) ? TimeCurrent() + expiration_seconds : 0;
               // Place multiple limit orders? Usually just 1 for limit.
               if(!trade.BuyLimit(g_UserLotSize * entry_count, limit_price, _Symbol, sl_price, tp1_price, ORDER_TIME_GTC, exp, "Client Limit BUY"))
                  Print("BuyLimit failed. Error: ", GetLastError());
              }
            else
              {
               // Execute multiple entries for market order
               for(int k=0; k<entry_count; k++)
                  if(!trade.Buy(g_UserLotSize, _Symbol, ask, sl_price, tp1_price, "Client Market BUY"))
                     Print("Buy failed. Error: ", GetLastError());
              }
           }
        }
      else
         if(recommended_order_type == "limit_short" || recommended_order_type == "market_short")
           {
            ClosePositions(POSITION_TYPE_BUY); // Hedge/Flip
            int open_sells = CountOpenPositions(POSITION_TYPE_SELL);
            int pending_sells = CountPendingOrders(ORDER_TYPE_SELL_LIMIT);

            // If forcing market, delete pending orders to allow execution
            if(g_ForceMarketOrders && pending_sells > 0)
              {
               Print("Force Market ON: Deleting pending sell limits to execute market order.");
               for(int i=OrdersTotal()-1; i>=0; i--)
                  if(OrderGetString(ORDER_SYMBOL)==_Symbol && OrderGetInteger(ORDER_MAGIC)==MagicNumber && OrderGetInteger(ORDER_TYPE)==ORDER_TYPE_SELL_LIMIT)
                     trade.OrderDelete(OrderGetTicket(i));
               pending_sells = 0;
              }

            // Execute if under max entries
            if(pending_sells == 0 && open_sells < g_UserMaxEntries)
              {
               // Check if Limit price is better than market (Sell Limit <= Bid), convert to Market
               bool force_market = (recommended_order_type == "limit_short" && limit_price > 0 && limit_price <= bid) || g_ForceMarketOrders;
               if(force_market && !g_ForceMarketOrders)
                  Print("Limit Sell Price ", limit_price, " <= Bid ", bid, ". Converting to Market Sell.");

               if(recommended_order_type == "limit_short" && limit_price > 0 && !force_market)
                 {
                  datetime exp = (expiration_seconds > 0) ? TimeCurrent() + expiration_seconds : 0;
                  if(!trade.SellLimit(g_UserLotSize * entry_count, limit_price, _Symbol, sl_price, tp1_price, ORDER_TIME_GTC, exp, "Client Limit SELL"))
                     Print("SellLimit failed. Error: ", GetLastError());
                 }
               else
                 {
                  // Execute multiple entries for market order
                  for(int k=0; k<entry_count; k++)
                     if(!trade.Sell(g_UserLotSize, _Symbol, bid, sl_price, tp1_price, "Client Market SELL"))
                        Print("Sell failed. Error: ", GetLastError());
                 }
              }
           }
     }
  }

//+------------------------------------------------------------------+
//| UTILITY FUNCTIONS                                                |
//+------------------------------------------------------------------+
void ClosePositions(ENUM_POSITION_TYPE direction)
  {
   for(int i = PositionsTotal() - 1; i >= 0; i--)
     {
      if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber && PositionGetInteger(POSITION_TYPE) == direction)
         trade.PositionClose(PositionGetTicket(i));
     }
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
int CountOpenPositions(ENUM_POSITION_TYPE direction)
  {
   int count = 0;
   for(int i = PositionsTotal() - 1; i >= 0; i--)
     {
      if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber && PositionGetInteger(POSITION_TYPE) == direction)
         count++;
     }
   return count;
  }

//+------------------------------------------------------------------+
//| Count Pending Orders                                             |
//+------------------------------------------------------------------+
int CountPendingOrders(ENUM_ORDER_TYPE type)
  {
   int count = 0;
   for(int i = OrdersTotal() - 1; i >= 0; i--)
     {
      ulong ticket = OrderGetTicket(i);
      if(ticket > 0)
        {
         if(OrderGetString(ORDER_SYMBOL) == _Symbol && OrderGetInteger(ORDER_MAGIC) == MagicNumber)
            if((ENUM_ORDER_TYPE)OrderGetInteger(ORDER_TYPE) == type)
               count++;
        }
     }
   return count;
  }

//+------------------------------------------------------------------+
//| Calculate Entry Count based on Conviction Score                  |
//+------------------------------------------------------------------+
int CalculateEntryCount(int conviction)
  {
   int min_conviction = 50;
   if(conviction < min_conviction)
      return 0;

// Map conviction 50-100 to 1-g_UserMaxEntries
   double ratio = (double)(conviction - min_conviction) / 50.0; // 0.0 to 1.0
   int count = 1 + (int)MathRound(ratio * (g_UserMaxEntries - 1));

   return MathMin(count, g_UserMaxEntries);
  }

// --- JSON Parsers ---
string GetJsonValue(string json, string key, bool is_string)
  {
   string search_key = is_string ? "\"" + key + "\":\"" : "\"" + key + "\":";
   string end_char = is_string ? "\"" : ",";
   int start_pos = StringFind(json, search_key);
   if(start_pos < 0)
      return "";
   start_pos += StringLen(search_key);
   int end_pos = -1;
   if(is_string)
      end_pos = StringFind(json, end_char, start_pos);
   else
     {
      int comma_pos = StringFind(json, ",", start_pos);
      int brace_pos = StringFind(json, "}", start_pos);
      if(comma_pos > 0 && brace_pos > 0)
         end_pos = MathMin(comma_pos, brace_pos);
      else
         end_pos = (comma_pos > 0) ? comma_pos : brace_pos;
     }
   if(end_pos < 0)
      return "";
   return StringSubstr(json, start_pos, end_pos - start_pos);
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
string GetNestedJsonValue(string json, string object_key, string value_key, bool is_string)
  {
   string search_object_key = "\"" + object_key + "\":{";
   int object_start_pos = StringFind(json, search_object_key);
   if(object_start_pos < 0)
      return "";
   object_start_pos += StringLen(search_object_key);
   int brace_count = 1;
   int object_end_pos = -1;
   for(int i = object_start_pos; i < StringLen(json); i++)
     {
      if(StringGetCharacter(json, i) == '{')
         brace_count++;
      if(StringGetCharacter(json, i) == '}')
         brace_count--;
      if(brace_count == 0)
        {
         object_end_pos = i;
         break;
        }
     }
   if(object_end_pos < 0)
      return "";
   string nested_json = StringSubstr(json, object_start_pos - 1, object_end_pos - (object_start_pos - 1) + 1);
   return GetJsonValue(nested_json, value_key, is_string);
  }

//+------------------------------------------------------------------+
//| Extracts a raw JSON object string by key                         |
//+------------------------------------------------------------------+
string GetJsonObject(string json, string key)
  {
   string search_key = "\"" + key + "\":{";
   int start_pos = StringFind(json, search_key);
   if(start_pos < 0)
      return "";

   start_pos += StringLen(search_key) - 1; // Points to '{'
   int brace_count = 0;
   int end_pos = -1;

   for(int i = start_pos; i < StringLen(json); i++)
     {
      if(StringGetCharacter(json, i) == '{')
         brace_count++;
      if(StringGetCharacter(json, i) == '}')
         brace_count--;
      if(brace_count == 0)
        {
         end_pos = i;
         break;
        }
     }

   if(end_pos < 0)
      return "";
   return StringSubstr(json, start_pos, end_pos - start_pos + 1);
  }

// --- Visuals ---
void CreateSignalArrow(long chart_ID, string name, datetime time, double price, int arrow_code, color clr)
  {
   if(ObjectFind(chart_ID, name) < 0)
     {
      ObjectCreate(chart_ID, name, OBJ_ARROW, 0, time, price);
      ObjectSetInteger(chart_ID, name, OBJPROP_ARROWCODE, arrow_code);
      ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, clr);
      ObjectSetInteger(chart_ID, name, OBJPROP_WIDTH, 1);
      ObjectSetInteger(chart_ID, name, OBJPROP_ANCHOR, (arrow_code == 233) ? ANCHOR_TOP : ANCHOR_BOTTOM);
     }
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void DrawServerOrderBlocks(long chart_ID, string json_response)
  {
   int start_key = StringFind(json_response, "\"orderBlocks\":[");
   if(start_key < 0)
      return;
   int start_arr = start_key + StringLen("\"orderBlocks\":[");
   int end_arr = StringFind(json_response, "]", start_arr);
   if(end_arr == -1)
      return;
   string arr_content = StringSubstr(json_response, start_arr, end_arr - start_arr);

   int current_pos = 0;
   while(current_pos < StringLen(arr_content))
     {
      int obj_start = StringFind(arr_content, "{", current_pos);
      if(obj_start < 0)
         break;
      int obj_end = StringFind(arr_content, "}", obj_start);
      if(obj_end < 0)
         break;
      string obj_json = StringSubstr(arr_content, obj_start, obj_end - obj_start + 1);

      double top = StringToDouble(GetJsonValue(obj_json, "top", false));
      bool is_bullish = (GetJsonValue(obj_json, "isBullish", false) == "true");
      string name = "XauBridge_SrvOB_" + DoubleToString(top, 2);

      if(ObjectFind(chart_ID, name) < 0)
        {
         ObjectCreate(chart_ID, name, OBJ_RECTANGLE, 0, TimeCurrent() - PeriodSeconds() * 500, top, TimeCurrent() + PeriodSeconds() * 100, StringToDouble(GetJsonValue(obj_json, "bottom", false)));
         color zone_color = is_bullish ? C'100,149,237' : C'255,160,122';
         ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, zone_color);
         ObjectSetInteger(chart_ID, name, OBJPROP_FILL, true);
         ObjectSetInteger(chart_ID, name, OBJPROP_WIDTH, 1);
         ObjectSetInteger(chart_ID, name, OBJPROP_RAY_RIGHT, true);
         ObjectSetInteger(chart_ID, name, OBJPROP_BACK, true);
         CreateChartLabel(chart_ID, name+"_Lbl", TimeCurrent(), top, is_bullish?"Bullish OB":"Bearish OB", zone_color, ANCHOR_LEFT_LOWER);
        }
      current_pos = obj_end + 1;
     }
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void DrawServerLiquidityZones(long chart_ID, string json_response)
  {
   int start_key = StringFind(json_response, "\"liquidityZones\":[");
   if(start_key < 0)
      return;
   int start_arr = start_key + StringLen("\"liquidityZones\":[");
   int end_arr = StringFind(json_response, "]", start_arr);
   if(end_arr == -1)
      return;
   string arr_content = StringSubstr(json_response, start_arr, end_arr - start_arr);

   int current_pos = 0;
   while(current_pos < StringLen(arr_content))
     {
      int obj_start = StringFind(arr_content, "{", current_pos);
      if(obj_start < 0)
         break;
      int obj_end = StringFind(arr_content, "}", obj_start);
      if(obj_end < 0)
         break;
      string obj_json = StringSubstr(arr_content, obj_start, obj_end - obj_start + 1);

      double top = StringToDouble(GetJsonValue(obj_json, "top", false));
      bool is_bullish = (GetJsonValue(obj_json, "isBullish", false) == "true");
      string name = "XauBridge_SrvLiq_" + DoubleToString(top, 2);

      if(ObjectFind(chart_ID, name) < 0)
        {
         ObjectCreate(chart_ID, name, OBJ_RECTANGLE, 0, TimeCurrent() - PeriodSeconds() * 500, top, TimeCurrent() + PeriodSeconds() * 100, StringToDouble(GetJsonValue(obj_json, "bottom", false)));
         color zone_color = is_bullish ? C'30,144,255' : C'255,69,0';
         ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, zone_color);
         ObjectSetInteger(chart_ID, name, OBJPROP_FILL, false);
         ObjectSetInteger(chart_ID, name, OBJPROP_WIDTH, 1);
         ObjectSetInteger(chart_ID, name, OBJPROP_RAY_RIGHT, true);
         ObjectSetInteger(chart_ID, name, OBJPROP_BACK, true);
         CreateChartLabel(chart_ID, name+"_Lbl", TimeCurrent(), top, is_bullish?"Liq Support":"Liq Resistance", zone_color, ANCHOR_LEFT_LOWER);
        }
      current_pos = obj_end + 1;
     }
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void DrawServerImbalances(long chart_ID, string json_response)
  {
   string key_name = "\"imbalanceZones\":[";
   int start_key = StringFind(json_response, key_name);
   if(start_key < 0)
     {
      key_name="\"imbalances\":[";
      start_key=StringFind(json_response, key_name);
     }
   if(start_key < 0)
      return;
   int start_arr = start_key + StringLen(key_name);
   int end_arr = StringFind(json_response, "]", start_arr);
   if(end_arr == -1)
      return;
   string arr_content = StringSubstr(json_response, start_arr, end_arr - start_arr);

   int current_pos = 0;
   while(current_pos < StringLen(arr_content))
     {
      int obj_start = StringFind(arr_content, "{", current_pos);
      if(obj_start < 0)
         break;
      int obj_end = StringFind(arr_content, "}", obj_start);
      if(obj_end < 0)
         break;
      string obj_json = StringSubstr(arr_content, obj_start, obj_end - obj_start + 1);

      long time_val = (long)StringToInteger(GetJsonValue(obj_json, "time", false));
      if(time_val==0)
         time_val = (long)StringToInteger(GetJsonValue(obj_json, "timestamp", false));
      double high_val = StringToDouble(GetJsonValue(obj_json, "high", false));
      if(high_val==0)
         high_val = StringToDouble(GetJsonValue(obj_json, "top", false));
      double low_val = StringToDouble(GetJsonValue(obj_json, "low", false));
      if(low_val==0)
         low_val = StringToDouble(GetJsonValue(obj_json, "bottom", false));
      string type_val = GetJsonValue(obj_json, "type", true);
      string is_bullish_str = GetJsonValue(obj_json, "isBullish", false);

      if(high_val > 0 && low_val > 0)
        {
         string name = "XauBridge_SrvFVG_" + (time_val>0 ? IntegerToString(time_val) : DoubleToString(high_val,2));
         bool is_bull = (StringFind(type_val, "bull") >= 0) || (is_bullish_str == "true");
         color fvg_color = is_bull ? C'60,179,113' : C'205,92,92';

         if(ObjectFind(chart_ID, name) < 0)
           {
            ObjectCreate(chart_ID, name, OBJ_RECTANGLE, 0, (datetime)(time_val>0?time_val:(TimeCurrent()-PeriodSeconds()*500)), high_val, TimeCurrent()+PeriodSeconds()*10, low_val);
            ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, fvg_color);
            ObjectSetInteger(chart_ID, name, OBJPROP_FILL, true);
            ObjectSetInteger(chart_ID, name, OBJPROP_BACK, true);
            ObjectSetInteger(chart_ID, name, OBJPROP_WIDTH, 0);
            ObjectSetInteger(chart_ID, name, OBJPROP_RAY_RIGHT, true);
           }
         CreateChartLabel(chart_ID, name+"_Lbl", TimeCurrent() + PeriodSeconds() * 5, high_val, is_bull?"Bullish FVG":"Bearish FVG", fvg_color, ANCHOR_LEFT_LOWER);
        }
      current_pos = obj_end + 1;
     }
  }

// --- Dashboard & UI (Condensed for brevity, logic identical to original) ---
double GetUiScale()
  {
   if(DashboardScale > 0.0)
      return DashboardScale;
   double s = (double)ChartGetInteger(ChartID(), CHART_WIDTH_IN_PIXELS) / 1440.0;
   return MathMin(1.5, MathMax(0.75, s));
  }
int ui(int px) { return (int)MathRound(px * GetUiScale()); }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void UpdateDashboard(string action, string reason, string tp, string sl, double current_spread, double current_balance, double current_pl, double next_lot_size, double conviction, string bias, string rr)
  {
   g_DashEntryType=action;
   g_DashAction=action;
   g_DashReason=reason;
   g_DashTP=tp;
   g_DashSL=sl;
   g_DashSpread=current_spread;
   g_DashBalance=current_balance;
   g_DashPL=current_pl;
   g_DashLot=next_lot_size;
   g_DashConviction=conviction;
   g_DashBias=bias;
   g_DashRR=rr;
   DrawDashboard();
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void DrawDashboard()
  {
   long chart_ID = ChartID();
   int panelW = ui(g_compactMode ? 250 : 380);
   int panelH = ui(g_compactMode ? 40 : 440); // Reduced height to minimize gap
   if(g_DashboardX == -1)
      g_DashboardX = ui(10);
   if(g_DashboardY == -1)
      g_DashboardY = ui(10);
   int x_pos = g_DashboardX;
   int y_pos = g_DashboardY;

   string panelBorder = "XauBridge_Panel_Border";
   if(ObjectFind(chart_ID, panelBorder) < 0)
     {
      ObjectCreate(chart_ID, panelBorder, OBJ_RECTANGLE_LABEL, 0, 0, 0);
      ObjectSetInteger(chart_ID, panelBorder, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID, panelBorder, OBJPROP_SELECTABLE, false);
      ObjectSetInteger(chart_ID, panelBorder, OBJPROP_BACK, false);
      ObjectSetInteger(chart_ID, panelBorder, OBJPROP_ZORDER, 100);
     }
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_XDISTANCE, x_pos);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_YDISTANCE, y_pos);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_XSIZE, panelW);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_YSIZE, panelH);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_BGCOLOR, (long)THEME_BG);
   ObjectSetInteger(chart_ID, panelBorder, OBJPROP_BORDER_COLOR, (long)THEME_BORDER);

   CreateLabel(chart_ID, "XauBridge_Title", x_pos + ui(10), y_pos + ui(8), "XAU SCALPER BRIDGE", THEME_BORDER, ui(12), "Segoe UI Semibold");
   CreateCollapseButton(chart_ID, x_pos + panelW - ui(25), y_pos + ui(8), g_compactMode);

   if(g_ShowSettings)
     {
      int settingsH = ui(440);
      ObjectSetInteger(chart_ID, panelBorder, OBJPROP_YSIZE, settingsH);
      DrawSettings(chart_ID, x_pos, y_pos, panelW, settingsH);
      return;
     }

   if(g_compactMode)
     {
      string sig = (StringFind(g_DashAction, "long") >= 0 ? "BUY" : (StringFind(g_DashAction,"short")>=0 ? "SELL":"WAIT"));
      color sig_col = (StringFind(g_DashAction,"long")>=0 ? THEME_ACCENT : (StringFind(g_DashAction,"short")>=0 ? THEME_ACCENT2 : THEME_MUTED));
      CreateLabel(chart_ID, "XauBridge_Compact_Text", x_pos + ui(10), y_pos + ui(24), sig + " | Conv: " + DoubleToString(g_DashConviction,1) + "%", sig_col, ui(11));
      ObjectDelete(chart_ID, "XauBridge_Signal");
      ObjectDelete(chart_ID, "XauBridge_Reason");
      ObjectDelete(chart_ID, "XauBridge_Reason2");
      ObjectDelete(chart_ID, "XauBridge_Balance");
      ObjectDelete(chart_ID, "XauBridge_Balance_Val");
      ObjectDelete(chart_ID, "XauBridge_Spread");
      ObjectDelete(chart_ID, "XauBridge_Spread_Val");
      ObjectDelete(chart_ID, "XauBridge_Current P/L");
      ObjectDelete(chart_ID, "XauBridge_Current P/L_Val");
      ObjectDelete(chart_ID, "XauBridge_Separator");
      ObjectsDeleteAll(chart_ID, "XauBridge_CB_");
      ObjectDelete(chart_ID, "XauBridge_CloseAll");
      ObjectDelete(chart_ID, "XauBridge_ZoneToggle");
      ObjectDelete(chart_ID, "XauBridge_ArrowToggle");
      ObjectsDeleteAll(chart_ID, "XauBridge_Lot");
      ObjectsDeleteAll(chart_ID, "XauBridge_Ent");
      ObjectDelete(chart_ID, "XauBridge_ForceMarketToggle");
      ObjectDelete(chart_ID, "XauBridge_Est. R:R");
      ObjectDelete(chart_ID, "XauBridge_Est. R:R_Val");
      ObjectDelete(chart_ID, "XauBridge_Stag. Timer");
      ObjectDelete(chart_ID, "XauBridge_Stag. Timer_Val");
      ObjectDelete(chart_ID, "XauBridge_Max Entries");
      ObjectDelete(chart_ID, "XauBridge_ConfigBtn");
     }
   else
     {
      ObjectDelete(chart_ID, "XauBridge_Compact_Text");
      int left_x = x_pos + ui(15), right_x = x_pos + ui(200), cur_y = y_pos + ui(40);

      string sig_text = (StringFind(g_DashAction, "long") >= 0 ? "SIGNAL: BUY" : (StringFind(g_DashAction,"short")>=0 ? "SIGNAL: SELL" : "SIGNAL: WAIT"));
      color sig_col = (StringFind(g_DashAction,"long")>=0 ? THEME_ACCENT : (StringFind(g_DashAction,"short")>=0 ? THEME_ACCENT2 : THEME_MUTED));
      CreateLabel(chart_ID, "XauBridge_Signal", left_x, cur_y, sig_text, sig_col, ui(16), "Segoe UI Semibold");
      cur_y += ui(30);

      string reason_full = "Reason: " + g_DashReason;
      int max_chars = 40;
      if(StringLen(reason_full) > max_chars)
        {
         int split_idx = -1;
         for(int i = max_chars; i > 0; i--)
            if(StringGetCharacter(reason_full, i) == ' ')
              {
               split_idx = i;
               break;
              }
         if(split_idx == -1)
            split_idx = max_chars;
         string line1 = StringSubstr(reason_full, 0, split_idx);
         string line2 = StringSubstr(reason_full, split_idx + 1);
         if(StringLen(line2) > max_chars)
            line2 = StringSubstr(line2, 0, max_chars - 3) + "...";
         CreateLabel(chart_ID, "XauBridge_Reason", left_x, cur_y, line1, THEME_TEXT, ui(10));
         CreateLabel(chart_ID, "XauBridge_Reason2", left_x, cur_y + ui(16), line2, THEME_TEXT, ui(10));
         cur_y += ui(38);
        }
      else
        {
         CreateLabel(chart_ID, "XauBridge_Reason", left_x, cur_y, reason_full, THEME_TEXT, ui(10));
         ObjectDelete(chart_ID, "XauBridge_Reason2");
         cur_y += ui(25);
        }

      string sep_name = "XauBridge_Separator";
      if(ObjectFind(chart_ID, sep_name) < 0)
        {
         ObjectCreate(chart_ID, sep_name, OBJ_RECTANGLE_LABEL, 0, 0, 0);
         ObjectSetInteger(chart_ID, sep_name, OBJPROP_CORNER, CORNER_LEFT_UPPER);
         ObjectSetInteger(chart_ID, sep_name, OBJPROP_BACK, false);
        }
      ObjectSetInteger(chart_ID, sep_name, OBJPROP_XDISTANCE, left_x);
      ObjectSetInteger(chart_ID, sep_name, OBJPROP_YDISTANCE, cur_y);
      ObjectSetInteger(chart_ID, sep_name, OBJPROP_XSIZE, panelW - ui(20));
      ObjectSetInteger(chart_ID, sep_name, OBJPROP_YSIZE, 1);
      ObjectSetInteger(chart_ID, sep_name, OBJPROP_BGCOLOR, THEME_MUTED);
      cur_y += ui(15);

      CreateDashboardRow(chart_ID, "Balance", DoubleToString(g_DashBalance,2), left_x, right_x, cur_y, THEME_MUTED, THEME_TEXT);
      cur_y += ui(28);
      CreateDashboardRow(chart_ID, "Spread", DoubleToString(g_DashSpread,1) + " pts", left_x, right_x, cur_y, THEME_MUTED, (g_DashSpread <= MaxSpreadPoints ? THEME_ACCENT : THEME_ACCENT2));
      cur_y += ui(28);

      CreateDashboardRow(chart_ID, "Est. R:R", g_DashRR, left_x, right_x, cur_y, THEME_MUTED, THEME_TEXT);
      cur_y += ui(28);

      string pl_string = (g_DashPL == 0.0 && PositionsTotal() == 0) ? "---" : DoubleToString(g_DashPL,2);
      color pl_color = (g_DashPL > 0) ? THEME_ACCENT : (g_DashPL < 0 ? THEME_ACCENT2 : THEME_TEXT);
      CreateDashboardRow(chart_ID, "Current P/L", pl_string, left_x, right_x, cur_y, THEME_MUTED, pl_color);
      cur_y += ui(32);

      color stag_color = THEME_TEXT;
      if(g_DashStagnationCountdown != "-" && g_DashStagnationCountdown != "OFF" && g_DashStagnationCountdown != "Safe")
         if(StringFind(g_DashStagnationCountdown, "00:") == 0)
            stag_color = THEME_ACCENT2;
      CreateDashboardRow(chart_ID, "Stag. Timer", g_DashStagnationCountdown, left_x, right_x, cur_y, THEME_MUTED, stag_color);
      cur_y += ui(28);

      DrawConvictionBar(chart_ID, left_x, cur_y, panelW - ui(20), 20, g_DashConviction);
      cur_y += ui(30);

      // --- New Controls ---
      CreateControlRow(chart_ID, "Lot Size", DoubleToString(g_UserLotSize, 2), left_x, cur_y, "XauBridge_LotMinus", "XauBridge_LotPlus");
      cur_y += ui(24);
      CreateControlRow(chart_ID, "Max Entries", IntegerToString(g_UserMaxEntries), left_x, cur_y, "XauBridge_EntMinus", "XauBridge_EntPlus");
      cur_y += ui(24);

      // New Force Market Button
      CreateForceMarketButton(chart_ID, left_x, cur_y);
      UpdateForceMarketButtonState();
      CreateConfigButton(chart_ID, left_x + ui(210), cur_y);
      cur_y += ui(30);

      int btn_y = y_pos + panelH - ui(32);
      CreateToggleButton(chart_ID, left_x, btn_y);
      CreateArrowToggleButton(chart_ID, left_x + ui(110), btn_y);
      UpdateButtonState();
      UpdateArrowButtonState();
      CreateCloseAllButton(chart_ID, x_pos + panelW - ui(100) - ui(10), btn_y);
     }
   ChartRedraw(chart_ID);
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateLabel(const long chart_ID, const string name, const int x, const int y, const string text, const color text_color, int font_size, string font)
  {
   if(ObjectFind(chart_ID, name)<0)
     {
      ObjectCreate(chart_ID, name, OBJ_LABEL,0,0,0);
      ObjectSetInteger(chart_ID,name,OBJPROP_CORNER,CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID,name,OBJPROP_SELECTABLE,false);
      ObjectSetInteger(chart_ID,name,OBJPROP_BACK,false);
     }
   ObjectSetInteger(chart_ID, name, OBJPROP_XDISTANCE, x);
   ObjectSetInteger(chart_ID, name, OBJPROP_YDISTANCE, y);
   ObjectSetString(chart_ID, name, OBJPROP_TEXT, text);
   ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, text_color);
   ObjectSetInteger(chart_ID, name, OBJPROP_FONTSIZE, font_size);
   ObjectSetString(chart_ID, name, OBJPROP_FONT, font);
   ObjectSetInteger(chart_ID, name, OBJPROP_ZORDER, 101);
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateDashboardRow(long chart_ID, string key, string value, int x_key, int x_val, int y, color clr_key, color clr_val)
  {
   CreateLabel(chart_ID, "XauBridge_"+key, x_key, y, key, clr_key, ui(11));
   CreateLabel(chart_ID, "XauBridge_"+key+"_Val", x_val, y, value, clr_val, ui(11));
  }

//+------------------------------------------------------------------+
//| Create Control Row with +/- Buttons                              |
//+------------------------------------------------------------------+
void CreateControlRow(long chart_ID, string key, string value, int x, int y, string btnMinus, string btnPlus)
  {
// Label
   CreateLabel(chart_ID, "XauBridge_"+key, x, y, key + ": " + value, THEME_TEXT, ui(10));

   int btnW = ui(20);
   int btnH = ui(18);
   int x_btns = x + ui(160);

// Minus Button
   if(ObjectFind(chart_ID, btnMinus) < 0)
     {
      ObjectCreate(chart_ID, btnMinus, OBJ_BUTTON, 0, 0, 0);
      ObjectSetInteger(chart_ID, btnMinus, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetString(chart_ID, btnMinus, OBJPROP_TEXT, "-");
      ObjectSetInteger(chart_ID, btnMinus, OBJPROP_ZORDER, 101);
     }
   ObjectSetInteger(chart_ID, btnMinus, OBJPROP_XDISTANCE, x_btns);
   ObjectSetInteger(chart_ID, btnMinus, OBJPROP_YDISTANCE, y);
   ObjectSetInteger(chart_ID, btnMinus, OBJPROP_XSIZE, btnW);
   ObjectSetInteger(chart_ID, btnMinus, OBJPROP_YSIZE, btnH);
   ObjectSetInteger(chart_ID, btnMinus, OBJPROP_BGCOLOR, C'60,60,60');
   ObjectSetInteger(chart_ID, btnMinus, OBJPROP_COLOR, C'255,255,255');

// Plus Button
   if(ObjectFind(chart_ID, btnPlus) < 0)
     {
      ObjectCreate(chart_ID, btnPlus, OBJ_BUTTON, 0, 0, 0);
      ObjectSetInteger(chart_ID, btnPlus, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetString(chart_ID, btnPlus, OBJPROP_TEXT, "+");
      ObjectSetInteger(chart_ID, btnPlus, OBJPROP_ZORDER, 101);
     }
   ObjectSetInteger(chart_ID, btnPlus, OBJPROP_XDISTANCE, x_btns + btnW + ui(5));
   ObjectSetInteger(chart_ID, btnPlus, OBJPROP_YDISTANCE, y);
   ObjectSetInteger(chart_ID, btnPlus, OBJPROP_XSIZE, btnW);
   ObjectSetInteger(chart_ID, btnPlus, OBJPROP_YSIZE, btnH);
   ObjectSetInteger(chart_ID, btnPlus, OBJPROP_BGCOLOR, C'60,60,60');
   ObjectSetInteger(chart_ID, btnPlus, OBJPROP_COLOR, C'255,255,255');
  }

//+------------------------------------------------------------------+
//| Create Toggle Row for Settings                                   |
//+------------------------------------------------------------------+
void CreateToggleRow(long chart_ID, string key, bool value, int x, int y, string btnName)
  {
   CreateLabel(chart_ID, "XauBridge_Set_"+key, x, y, key, THEME_TEXT, ui(10));

   if(ObjectFind(chart_ID, btnName) < 0)
     {
      ObjectCreate(chart_ID, btnName, OBJ_BUTTON, 0, 0, 0);
      ObjectSetInteger(chart_ID, btnName, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID, btnName, OBJPROP_ZORDER, 102);
     }
   ObjectSetInteger(chart_ID, btnName, OBJPROP_XDISTANCE, x + ui(160));
   ObjectSetInteger(chart_ID, btnName, OBJPROP_YDISTANCE, y);
   ObjectSetInteger(chart_ID, btnName, OBJPROP_XSIZE, ui(60));
   ObjectSetInteger(chart_ID, btnName, OBJPROP_YSIZE, ui(18));
   ObjectSetString(chart_ID, btnName, OBJPROP_TEXT, value ? "ON" : "OFF");
   ObjectSetInteger(chart_ID, btnName, OBJPROP_BGCOLOR, value ? C'47,79,79' : C'60,60,60');
   ObjectSetInteger(chart_ID, btnName, OBJPROP_COLOR, C'255,255,255');
  }

//+------------------------------------------------------------------+
//| Draw Settings Panel                                              |
//+------------------------------------------------------------------+
void DrawSettings(long chart_ID, int x, int y, int w, int h)
  {
// Clear main dashboard elements that might overlap
   ObjectDelete(chart_ID, "XauBridge_Signal");
   ObjectDelete(chart_ID, "XauBridge_Reason");
   ObjectDelete(chart_ID, "XauBridge_Reason2");
   ObjectDelete(chart_ID, "XauBridge_Separator");
   ObjectsDeleteAll(chart_ID, "XauBridge_CB_");
   ObjectsDeleteAll(chart_ID, "XauBridge_Lot");
   ObjectDelete(chart_ID, "XauBridge_Lot Size"); // Explicit delete
   ObjectsDeleteAll(chart_ID, "XauBridge_Ent");
   ObjectDelete(chart_ID, "XauBridge_Max Entries"); // Explicit delete
   ObjectDelete(chart_ID, "XauBridge_ForceMarketToggle");
   ObjectDelete(chart_ID, "XauBridge_CloseAll");
   ObjectDelete(chart_ID, "XauBridge_ConfigBtn");
   ObjectDelete(chart_ID, "XauBridge_ZoneToggle");
   ObjectDelete(chart_ID, "XauBridge_ArrowToggle");
   ObjectDelete(chart_ID, "XauBridge_Balance");
   ObjectDelete(chart_ID, "XauBridge_Balance_Val");
   ObjectDelete(chart_ID, "XauBridge_Spread");
   ObjectDelete(chart_ID, "XauBridge_Spread_Val");
   ObjectDelete(chart_ID, "XauBridge_Est. R:R");
   ObjectDelete(chart_ID, "XauBridge_Est. R:R_Val");
   ObjectDelete(chart_ID, "XauBridge_Current P/L");
   ObjectDelete(chart_ID, "XauBridge_Current P/L_Val");
   ObjectDelete(chart_ID, "XauBridge_Stag. Timer");
   ObjectDelete(chart_ID, "XauBridge_Stag. Timer_Val");

   int cur_y = y + ui(40);
   int left_x = x + ui(15);

   CreateLabel(chart_ID, "XauBridge_SetTitle", left_x, cur_y, "CONFIGURATION", THEME_ACCENT, ui(12), "Segoe UI Semibold");
   cur_y += ui(30);

   CreateToggleRow(chart_ID, "Scalp Mode", g_UserScalpMode, left_x, cur_y, "XauBridge_SetScalp");
   cur_y += ui(25);
   CreateToggleRow(chart_ID, "Trailing SL", g_UserTrailingSL, left_x, cur_y, "XauBridge_SetTrail");
   cur_y += ui(25);
   CreateControlRow(chart_ID, "Stag. Secs", IntegerToString(g_UserStagnationSeconds), left_x, cur_y, "XauBridge_SetStagMinus", "XauBridge_SetStagPlus");
   cur_y += ui(25);
   CreateToggleRow(chart_ID, "Show Liquidity", g_ShowLiq, left_x, cur_y, "XauBridge_SetLiq");
   cur_y += ui(25);
   CreateToggleRow(chart_ID, "Show FVG", g_ShowFVG, left_x, cur_y, "XauBridge_SetFVG");
   cur_y += ui(25);
   CreateToggleRow(chart_ID, "Show OrderBlocks", g_ShowOB, left_x, cur_y, "XauBridge_SetOB");
   cur_y += ui(25);

// Back Button
   string btnBack = "XauBridge_SetBack";
   if(ObjectFind(chart_ID, btnBack) < 0)
     {
      ObjectCreate(chart_ID, btnBack, OBJ_BUTTON, 0, 0, 0);
      ObjectSetInteger(chart_ID, btnBack, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID, btnBack, OBJPROP_ZORDER, 102);
      ObjectSetString(chart_ID, btnBack, OBJPROP_TEXT, "Back");
      ObjectSetInteger(chart_ID, btnBack, OBJPROP_BGCOLOR, C'60,60,60');
      ObjectSetInteger(chart_ID, btnBack, OBJPROP_COLOR, C'255,255,255');
      ObjectSetInteger(chart_ID, btnBack, OBJPROP_FONTSIZE, ui(9));
     }
   ObjectSetInteger(chart_ID, btnBack, OBJPROP_XDISTANCE, x + w - ui(100) - ui(10));
   ObjectSetInteger(chart_ID, btnBack, OBJPROP_YDISTANCE, y + h - ui(32));
   ObjectSetInteger(chart_ID, btnBack, OBJPROP_XSIZE, ui(100));
   ObjectSetInteger(chart_ID, btnBack, OBJPROP_YSIZE, ui(24));

// Reset Button
   string btnReset = "XauBridge_SetReset";
   if(ObjectFind(chart_ID, btnReset) < 0)
     {
      ObjectCreate(chart_ID, btnReset, OBJ_BUTTON, 0, 0, 0);
      ObjectSetInteger(chart_ID, btnReset, OBJPROP_CORNER, CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID, btnReset, OBJPROP_ZORDER, 102);
      ObjectSetString(chart_ID, btnReset, OBJPROP_TEXT, "Reset");
      ObjectSetInteger(chart_ID, btnReset, OBJPROP_BGCOLOR, C'178,34,34'); // Firebrick
      ObjectSetInteger(chart_ID, btnReset, OBJPROP_COLOR, C'255,255,255');
      ObjectSetInteger(chart_ID, btnReset, OBJPROP_FONTSIZE, ui(9));
     }
   ObjectSetInteger(chart_ID, btnReset, OBJPROP_XDISTANCE, x + ui(15));
   ObjectSetInteger(chart_ID, btnReset, OBJPROP_YDISTANCE, y + h - ui(32));
   ObjectSetInteger(chart_ID, btnReset, OBJPROP_XSIZE, ui(80));
   ObjectSetInteger(chart_ID, btnReset, OBJPROP_YSIZE, ui(24));
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void DrawConvictionBar(long chart_ID, int x, int y, int totalWidthPx, int heightPx, double convictionPercent)
  {
   string bg="XauBridge_CB_BG", fg="XauBridge_CB_FG", txt="XauBridge_CB_T";
   int w_bg=totalWidthPx, h=ui(heightPx);
   double p=MathMax(0,MathMin(100,convictionPercent));
   int fg_w=(int)MathRound(w_bg*(p/100.0));

   if(ObjectFind(chart_ID,bg)<0)
      ObjectCreate(chart_ID,bg,OBJ_RECTANGLE_LABEL,0,0,0);
   ObjectSetInteger(chart_ID,bg,OBJPROP_XDISTANCE,x);
   ObjectSetInteger(chart_ID,bg,OBJPROP_YDISTANCE,y);
   ObjectSetInteger(chart_ID,bg,OBJPROP_XSIZE,w_bg);
   ObjectSetInteger(chart_ID,bg,OBJPROP_YSIZE,h);
   ObjectSetInteger(chart_ID,bg,OBJPROP_BGCOLOR,THEME_MUTED);
   ObjectSetInteger(chart_ID,bg,OBJPROP_CORNER,CORNER_LEFT_UPPER);
   ObjectSetInteger(chart_ID,bg,OBJPROP_BORDER_COLOR,(long)THEME_BORDER);
   ObjectSetInteger(chart_ID,bg,OBJPROP_ZORDER, 101);

   if(ObjectFind(chart_ID,fg)<0)
      ObjectCreate(chart_ID,fg,OBJ_RECTANGLE_LABEL,0,0,0);
   ObjectSetInteger(chart_ID,fg,OBJPROP_XDISTANCE,x);
   ObjectSetInteger(chart_ID,fg,OBJPROP_YDISTANCE,y);
   ObjectSetInteger(chart_ID,fg,OBJPROP_XSIZE,fg_w);
   ObjectSetInteger(chart_ID,fg,OBJPROP_YSIZE,h);
   ObjectSetInteger(chart_ID,fg,OBJPROP_BGCOLOR,(p>=70?THEME_ACCENT:(p>=40?THEME_BORDER:THEME_ACCENT2)));
   ObjectSetInteger(chart_ID,fg,OBJPROP_CORNER,CORNER_LEFT_UPPER);
   ObjectSetInteger(chart_ID,fg,OBJPROP_ZORDER, 101);

   CreateLabel(chart_ID,txt,x+6,y+ui(2),"Conviction: "+DoubleToString(p,0)+"%",THEME_TEXT,ui(8));
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateChartLabel(long chart_ID, string name, datetime time, double price, string text, color clr, int anchor)
  {
   if(ObjectFind(chart_ID, name)<0)
     {
      ObjectCreate(chart_ID, name, OBJ_TEXT,0,time,price);
      ObjectSetInteger(chart_ID,name,OBJPROP_ANCHOR,anchor);
     }
   else
      ObjectMove(chart_ID, name, 0, time, price);
   ObjectSetString(chart_ID, name, OBJPROP_TEXT, text);
   ObjectSetInteger(chart_ID, name, OBJPROP_COLOR, clr);
   ObjectSetInteger(chart_ID, name, OBJPROP_FONTSIZE, 11);
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateToggleButton(long chart_ID, int x, int y)
  {
   string name="XauBridge_ZoneToggle";
   if(ObjectFind(chart_ID,name)<0)
     {
      ObjectCreate(chart_ID,name,OBJ_BUTTON,0,0,0);
      ObjectSetInteger(chart_ID,name,OBJPROP_CORNER,CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID,name,OBJPROP_ZORDER, 101);
     }
   ObjectSetInteger(chart_ID,name,OBJPROP_XDISTANCE,x);
   ObjectSetInteger(chart_ID,name,OBJPROP_YDISTANCE,y);
   ObjectSetInteger(chart_ID,name,OBJPROP_XSIZE,ui(100));
   ObjectSetInteger(chart_ID,name,OBJPROP_YSIZE,ui(24));
   ObjectSetInteger(chart_ID,name,OBJPROP_FONTSIZE,ui(9));
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void UpdateButtonState()
  {
   string name="XauBridge_ZoneToggle";
   ObjectSetString(0,name,OBJPROP_TEXT,"Zones: "+(g_ShowZones?"ON":"OFF"));
   ObjectSetInteger(0,name,OBJPROP_BGCOLOR,(g_ShowZones?C'47,79,79':C'139,0,0'));
   ObjectSetInteger(0,name,OBJPROP_COLOR,THEME_TEXT);
   ObjectSetInteger(0,name,OBJPROP_STATE,g_ShowZones);
  }

//+------------------------------------------------------------------+
//| Create Force Market Toggle Button                                |
//+------------------------------------------------------------------+
void CreateForceMarketButton(long chart_ID, int x, int y)
  {
   string name="XauBridge_ForceMarketToggle";
   if(ObjectFind(chart_ID,name)<0)
     {
      ObjectCreate(chart_ID,name,OBJ_BUTTON,0,0,0);
      ObjectSetInteger(chart_ID,name,OBJPROP_CORNER,CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID,name,OBJPROP_ZORDER, 101);
      ObjectSetString(chart_ID,name,OBJPROP_TOOLTIP,"When ON, converts all incoming Limit Order signals to immediate Market Orders.");
     }
   ObjectSetInteger(chart_ID,name,OBJPROP_XDISTANCE,x);
   ObjectSetInteger(chart_ID,name,OBJPROP_YDISTANCE,y);
   ObjectSetInteger(chart_ID,name,OBJPROP_XSIZE,ui(200));
   ObjectSetInteger(chart_ID,name,OBJPROP_YSIZE,ui(24));
   ObjectSetInteger(chart_ID,name,OBJPROP_FONTSIZE,ui(9));
  }

//+------------------------------------------------------------------+
//| Update Force Market Button State                                 |
//+------------------------------------------------------------------+
void UpdateForceMarketButtonState()
  {
   string name="XauBridge_ForceMarketToggle";
   ObjectSetString(0,name,OBJPROP_TEXT,"Force Market Orders: "+(g_ForceMarketOrders?"ON":"OFF"));
   ObjectSetInteger(0,name,OBJPROP_BGCOLOR,(g_ForceMarketOrders?C'218,112,214':C'47,79,79')); // Orchid when ON
   ObjectSetInteger(0,name,OBJPROP_COLOR,THEME_TEXT);
   ObjectSetInteger(0,name,OBJPROP_STATE,g_ForceMarketOrders);
  }

//+------------------------------------------------------------------+
//| Create Arrow Toggle Button                                       |
//+------------------------------------------------------------------+
void CreateArrowToggleButton(long chart_ID, int x, int y)
  {
   string name="XauBridge_ArrowToggle";
   if(ObjectFind(chart_ID,name)<0)
     {
      ObjectCreate(chart_ID,name,OBJ_BUTTON,0,0,0);
      ObjectSetInteger(chart_ID,name,OBJPROP_CORNER,CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID,name,OBJPROP_ZORDER, 101);
     }
   ObjectSetInteger(chart_ID,name,OBJPROP_XDISTANCE,x);
   ObjectSetInteger(chart_ID,name,OBJPROP_YDISTANCE,y);
   ObjectSetInteger(chart_ID,name,OBJPROP_XSIZE,ui(100));
   ObjectSetInteger(chart_ID,name,OBJPROP_YSIZE,ui(24));
   ObjectSetInteger(chart_ID,name,OBJPROP_FONTSIZE,ui(9));
  }

//+------------------------------------------------------------------+
//| Update Arrow Button State                                        |
//+------------------------------------------------------------------+
void UpdateArrowButtonState()
  {
   string name="XauBridge_ArrowToggle";
   ObjectSetString(0,name,OBJPROP_TEXT,"Entry: "+(g_ShowArrows?"ON":"OFF"));
   ObjectSetInteger(0,name,OBJPROP_BGCOLOR,(g_ShowArrows?C'47,79,79':C'139,0,0'));
   ObjectSetInteger(0,name,OBJPROP_COLOR,THEME_TEXT);
   ObjectSetInteger(0,name,OBJPROP_STATE,g_ShowArrows);
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateCollapseButton(long chart_ID, int x, int y, bool is_collapsed)
  {
   string name="XauBridge_CollapseToggle";
   if(ObjectFind(chart_ID,name)<0)
     {
      ObjectCreate(chart_ID,name,OBJ_BUTTON,0,0,0);
      ObjectSetInteger(chart_ID,name,OBJPROP_CORNER,CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID,name,OBJPROP_ZORDER, 101);
     }
   ObjectSetInteger(chart_ID,name,OBJPROP_XDISTANCE,x);
   ObjectSetInteger(chart_ID,name,OBJPROP_YDISTANCE,y);
   ObjectSetInteger(chart_ID,name,OBJPROP_XSIZE,ui(20));
   ObjectSetInteger(chart_ID,name,OBJPROP_YSIZE,ui(16));
   ObjectSetString(chart_ID,name,OBJPROP_TEXT,is_collapsed?"+":"-");
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CreateCloseAllButton(long chart_ID, int x, int y)
  {
   string name="XauBridge_CloseAll";
   if(ObjectFind(chart_ID,name)<0)
     {
      ObjectCreate(chart_ID,name,OBJ_BUTTON,0,0,0);
      ObjectSetInteger(chart_ID,name,OBJPROP_CORNER,CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID,name,OBJPROP_ZORDER, 101);
     }
   ObjectSetInteger(chart_ID,name,OBJPROP_XDISTANCE,x);
   ObjectSetInteger(chart_ID,name,OBJPROP_YDISTANCE,y);
   ObjectSetInteger(chart_ID,name,OBJPROP_XSIZE,ui(100));
   ObjectSetInteger(chart_ID,name,OBJPROP_YSIZE,ui(24));
   ObjectSetString(chart_ID,name,OBJPROP_TEXT,"Close All");
   ObjectSetInteger(chart_ID,name,OBJPROP_BGCOLOR,C'220,53,69');
   ObjectSetInteger(chart_ID,name,OBJPROP_COLOR,C'255,255,255');
   ObjectSetInteger(chart_ID,name,OBJPROP_FONTSIZE,ui(9));
  }

//+------------------------------------------------------------------+
//| Create Config Button                                             |
//+------------------------------------------------------------------+
void CreateConfigButton(long chart_ID, int x, int y)
  {
   string name="XauBridge_ConfigBtn";
   if(ObjectFind(chart_ID,name)<0)
     {
      ObjectCreate(chart_ID,name,OBJ_BUTTON,0,0,0);
      ObjectSetInteger(chart_ID,name,OBJPROP_CORNER,CORNER_LEFT_UPPER);
      ObjectSetInteger(chart_ID,name,OBJPROP_ZORDER, 101);
     }
   ObjectSetInteger(chart_ID,name,OBJPROP_XDISTANCE,x);
   ObjectSetInteger(chart_ID,name,OBJPROP_YDISTANCE,y);
   ObjectSetInteger(chart_ID,name,OBJPROP_XSIZE,ui(80));
   ObjectSetInteger(chart_ID,name,OBJPROP_YSIZE,ui(24));
   ObjectSetString(chart_ID,name,OBJPROP_TEXT,"Config");
   ObjectSetInteger(chart_ID,name,OBJPROP_BGCOLOR,C'105,105,105');
   ObjectSetInteger(chart_ID,name,OBJPROP_COLOR,C'255,255,255');
   ObjectSetInteger(chart_ID,name,OBJPROP_FONTSIZE,ui(9));
  }

//+------------------------------------------------------------------+
//|                                                                  |
//+------------------------------------------------------------------+
void CloseAllTrades()
  {
   for(int i=PositionsTotal()-1; i>=0; i--)
     {
      if(PositionGetInteger(POSITION_MAGIC)==MagicNumber && PositionGetSymbol(i)==_Symbol)
         trade.PositionClose(PositionGetTicket(i));
     }
  }

//+------------------------------------------------------------------+
//| Manage Trailing Stop Loss                                        |
//+------------------------------------------------------------------+
void ManageTrailingSL()
  {
   double point = SymbolInfoDouble(_Symbol, SYMBOL_POINT);
   long min_rem_time = -1;
   bool trailing_active = g_UserTrailingSL;

// Clean up old stagnation lines to handle closed trades
   ObjectsDeleteAll(0, "XauBridge_StagLine_");

   for(int i = PositionsTotal() - 1; i >= 0; i--)
     {
      if(PositionGetSymbol(i) == _Symbol && PositionGetInteger(POSITION_MAGIC) == MagicNumber)
        {
         ulong ticket = PositionGetTicket(i);

         // 1. Hard Max Trade Lifetime (Scalp Mode)
         if(g_UserScalpMode && MaxTradeLifetimeSeconds > 0)
           {
            if(TimeCurrent() - (datetime)PositionGetInteger(POSITION_TIME) > MaxTradeLifetimeSeconds)
              {
               trade.PositionClose(ticket);
               continue;
              }
           }

         if(!trailing_active)
            continue;

         ENUM_POSITION_TYPE type = (ENUM_POSITION_TYPE)PositionGetInteger(POSITION_TYPE);
         double open_price = PositionGetDouble(POSITION_PRICE_OPEN);
         double current_sl = PositionGetDouble(POSITION_SL);
         double current_tp = PositionGetDouble(POSITION_TP);
         double current_price = (type == POSITION_TYPE_BUY) ? SymbolInfoDouble(_Symbol, SYMBOL_BID) : SymbolInfoDouble(_Symbol, SYMBOL_ASK);

         double start_dist = 0.0;
         double trail_dist = 0.0;
         double step_dist  = 0.0;

         // Break Even calculations
         double be_trigger = BreakEvenTriggerPips * 10.0 * point;
         double be_offset  = BreakEvenOffsetPips * 10.0 * point;

         // Dynamic Stagnation Calculation
         int limit_seconds = g_UserStagnationSeconds;
         if(UseDynamicStagnation && g_ServerATR > 0)
           {
            // Estimate time to reach BE based on ATR (assuming M1 ATR)
            // Time = (Distance / Speed) * Mult. Speed = ATR/60.
            double expected_seconds = (be_trigger / g_ServerATR) * 60.0;
            limit_seconds = (int)(expected_seconds * StagnationTimeMult);
            if(limit_seconds < 30)
               limit_seconds = 30; // Minimum 30s
           }

         // Countdown Logic for Dashboard
         if(UseStagnationSL)
           {
            double profit_for_calc = (type == POSITION_TYPE_BUY) ? (current_price - open_price) : (open_price - current_price);
            if(profit_for_calc <= 0) // Only countdown if not in profit
              {
               datetime open_time = (datetime)PositionGetInteger(POSITION_TIME);
               long elapsed = TimeCurrent() - open_time;
               long rem = limit_seconds - elapsed;
               if(min_rem_time == -1 || rem < min_rem_time)
                  min_rem_time = rem;
              }
           }

         if(UseAtrTrailing && g_ServerATR > 0)
           {
            start_dist = g_ServerATR * AtrMultStart;
            trail_dist = g_ServerATR * AtrMultDist;
            step_dist  = g_ServerATR * AtrMultStep;
           }
         else
           {
            // 1 Pip = 10 Points
            start_dist = TrailingStartPips * 10.0 * point;
            trail_dist = TrailingDistPips * 10.0 * point;
            step_dist  = TrailingStepPips * 10.0 * point;
           }

         if(type == POSITION_TYPE_BUY)
           {
            double profit_level = current_price - open_price;

            // 1. Break Even Logic
            if(profit_level > be_trigger)
              {
               double be_price = open_price + be_offset;
               if(current_sl < be_price - point) // Move to BE if not already there
                 {
                  if(trade.PositionModify(ticket, be_price, current_tp))
                     current_sl = be_price;
                 }
              }

            // 2. Trailing Logic
            if(profit_level > start_dist)
              {
               double new_sl = current_price - trail_dist;
               if(new_sl > current_sl + step_dist)
                  trade.PositionModify(ticket, new_sl, current_tp);
              }

            // 3. Stagnation Logic (Time-Based Risk Reduction)
            if(UseStagnationSL)
              {
               datetime open_time = (datetime)PositionGetInteger(POSITION_TIME);

               // Visual: Draw projected SL line
               double current_risk_val = open_price - current_sl;
               if(current_risk_val > 0)
                 {
                  double new_risk_val = current_risk_val * (1.0 - (StagnationRiskReducePct / 100.0));
                  double projected_sl = open_price - new_risk_val;

                  string line_name = "XauBridge_StagLine_" + IntegerToString(ticket);
                  if(ObjectFind(0, line_name) < 0)
                    {
                     ObjectCreate(0, line_name, OBJ_TREND, 0, 0, 0);
                     ObjectSetInteger(0, line_name, OBJPROP_COLOR, clrGray);
                     ObjectSetInteger(0, line_name, OBJPROP_STYLE, STYLE_DOT);
                     ObjectSetInteger(0, line_name, OBJPROP_RAY_RIGHT, true);
                     ObjectSetInteger(0, line_name, OBJPROP_WIDTH, 1);
                     ObjectSetInteger(0, line_name, OBJPROP_SELECTABLE, false);
                     ObjectSetString(0, line_name, OBJPROP_TOOLTIP, "Stagnation SL Target");
                    }
                  datetime trigger_time = open_time + limit_seconds;
                  ObjectMove(0, line_name, 0, trigger_time, projected_sl);
                  ObjectMove(0, line_name, 1, trigger_time + PeriodSeconds()*30, projected_sl);
                 }

               if(TimeCurrent() - open_time > limit_seconds && profit_level <= 0) // Only trigger if not in profit
                 {
                  // Calculate reduced risk distance
                  double current_risk = open_price - current_sl;
                  if(current_risk > 0)
                    {
                     double new_risk = current_risk * (1.0 - (StagnationRiskReducePct / 100.0));
                     double new_sl = open_price - new_risk;

                     // Only modify if tightening and price allows
                     if(new_sl > current_sl + point)
                       {
                        // Ensure we don't place SL above current price (immediate stop out)
                        if(new_sl < current_price - (SymbolInfoInteger(_Symbol, SYMBOL_SPREAD) * point))
                           trade.PositionModify(ticket, new_sl, current_tp);
                       }
                    }
                 }
              }
           }
         else
            if(type == POSITION_TYPE_SELL)
              {
               double profit_level = open_price - current_price;

               // 1. Break Even Logic
               if(profit_level > be_trigger)
                 {
                  double be_price = open_price - be_offset;
                  if(current_sl == 0 || current_sl > be_price + point) // Move to BE if not already there
                    {
                     if(trade.PositionModify(ticket, be_price, current_tp))
                        current_sl = be_price;
                    }
                 }

               // 2. Trailing Logic
               if(profit_level > start_dist)
                 {
                  double new_sl = current_price + trail_dist;
                  if(current_sl == 0 || new_sl < current_sl - step_dist)
                     trade.PositionModify(ticket, new_sl, current_tp);
                 }

               // 3. Stagnation Logic (Time-Based Risk Reduction)
               if(UseStagnationSL)
                 {
                  datetime open_time = (datetime)PositionGetInteger(POSITION_TIME);

                  // Visual: Draw projected SL line
                  double current_risk_val = current_sl - open_price;
                  if(current_risk_val > 0)
                    {
                     double new_risk_val = current_risk_val * (1.0 - (StagnationRiskReducePct / 100.0));
                     double projected_sl = open_price + new_risk_val;

                     string line_name = "XauBridge_StagLine_" + IntegerToString(ticket);
                     if(ObjectFind(0, line_name) < 0)
                       {
                        ObjectCreate(0, line_name, OBJ_TREND, 0, 0, 0);
                        ObjectSetInteger(0, line_name, OBJPROP_COLOR, clrGray);
                        ObjectSetInteger(0, line_name, OBJPROP_STYLE, STYLE_DOT);
                        ObjectSetInteger(0, line_name, OBJPROP_RAY_RIGHT, true);
                        ObjectSetInteger(0, line_name, OBJPROP_WIDTH, 1);
                        ObjectSetInteger(0, line_name, OBJPROP_SELECTABLE, false);
                        ObjectSetString(0, line_name, OBJPROP_TOOLTIP, "Stagnation SL Target");
                       }
                     datetime trigger_time = open_time + limit_seconds;
                     ObjectMove(0, line_name, 0, trigger_time, projected_sl);
                     ObjectMove(0, line_name, 1, trigger_time + PeriodSeconds()*30, projected_sl);
                    }

                  if(TimeCurrent() - open_time > limit_seconds && profit_level <= 0) // Only trigger if not in profit
                    {
                     double current_risk = current_sl - open_price;
                     if(current_risk > 0)
                       {
                        double new_risk = current_risk * (1.0 - (StagnationRiskReducePct / 100.0));
                        double new_sl = open_price + new_risk;

                        if(current_sl == 0 || new_sl < current_sl - point)
                          {
                           // Ensure we don't place SL below current price
                           if(new_sl > current_price + (SymbolInfoInteger(_Symbol, SYMBOL_SPREAD) * point))
                              trade.PositionModify(ticket, new_sl, current_tp);
                          }
                       }
                    }
                 }
              }
        }
     }

// Update Global Dashboard Variable
   if(UseStagnationSL && trailing_active)
     {
      if(min_rem_time != -1)
        {
         if(min_rem_time <= 0)
            g_DashStagnationCountdown = "Triggering...";
         else
           {
            string m_str = (min_rem_time/60 < 10) ? "0"+IntegerToString(min_rem_time/60) : IntegerToString(min_rem_time/60);
            string s_str = (min_rem_time%60 < 10) ? "0"+IntegerToString(min_rem_time%60) : IntegerToString(min_rem_time%60);
            g_DashStagnationCountdown = m_str + ":" + s_str;
           }
        }
      else
         g_DashStagnationCountdown = "Safe";
     }
   else
      g_DashStagnationCountdown = "OFF";
  }

//+------------------------------------------------------------------+
//| Manage Pending Orders (Expiration)                               |
//+------------------------------------------------------------------+
void ManagePendingOrders()
  {
   if(LimitOrderExpirationSeconds <= 0)
      return;

   for(int i = OrdersTotal() - 1; i >= 0; i--)
     {
      ulong ticket = OrderGetTicket(i);
      if(ticket > 0)
        {
         if(OrderGetString(ORDER_SYMBOL) == _Symbol && OrderGetInteger(ORDER_MAGIC) == MagicNumber)
           {
            long setup_time = OrderGetInteger(ORDER_TIME_SETUP);
            if(TimeCurrent() - setup_time > LimitOrderExpirationSeconds)
              {
               Print("Deleting expired pending order #", ticket);
               trade.OrderDelete(ticket);
              }
           }
        }
     }
  }

// Event Handling
void OnChartEvent(const int id, const long &lparam, const double &dparam, const string &sparam)
  {
   if(id==CHARTEVENT_MOUSE_MOVE)
     {
      if((int)StringToInteger(sparam)==1)   // Click Drag
        {
         if(!g_IsDragging)
           {
            if(ObjectFind(0,"XauBridge_Panel_Border")>=0 && (int)lparam>=g_DashboardX && (int)lparam<=g_DashboardX+ui(300) && (int)dparam>=g_DashboardY && (int)dparam<=g_DashboardY+ui(50))
              {
               g_IsDragging=true;
               g_DragOffsetX=(int)lparam-g_DashboardX;
               g_DragOffsetY=(int)dparam-g_DashboardY;
               ChartSetInteger(0,CHART_MOUSE_SCROLL,false);
              }
           }
         else
           {
            g_DashboardX=(int)lparam-g_DragOffsetX;
            g_DashboardY=(int)dparam-g_DragOffsetY;
            DrawDashboard();
           }
        }
      else
        {
         g_IsDragging=false;
         ChartSetInteger(0,CHART_MOUSE_SCROLL,true);
        }
     }
   if(id==CHARTEVENT_OBJECT_CLICK)
     {
      if(sparam=="XauBridge_ZoneToggle")
        {
         g_ShowZones=!g_ShowZones;
         UpdateButtonState();
         if(!g_ShowZones)
           {
            ObjectsDeleteAll(0,"XauBridge_SrvFVG_");
            ObjectsDeleteAll(0,"XauBridge_SrvLiq_");
           ObjectsDeleteAll(0,"XauBridge_SrvOB_");
           }
       else
         {
          if(g_ShowFVG) DrawServerImbalances(0, g_SignalState.jsonResponse);
          if(g_ShowLiq) DrawServerLiquidityZones(0, g_SignalState.jsonResponse);
          if(g_ShowOB) DrawServerOrderBlocks(0, g_SignalState.jsonResponse);
         }
         ChartRedraw();
        }
      if(sparam=="XauBridge_ArrowToggle")
        {
         g_ShowArrows=!g_ShowArrows;
         UpdateArrowButtonState();
         if(!g_ShowArrows)
            ObjectsDeleteAll(0,"XauBridge_Arrow_");
         ChartRedraw();
        }
      if(sparam=="XauBridge_CollapseToggle")
        {
         g_compactMode=!g_compactMode;
         DrawDashboard();
        }
      if(sparam=="XauBridge_CloseAll")
        {
         if(MessageBox("Close ALL trades?","Confirm",MB_YESNO|MB_ICONQUESTION)==IDYES)
            CloseAllTrades();
        }
      if(sparam=="XauBridge_ConfigBtn")
        {
         g_ShowSettings = true;
         DrawDashboard();
        }
      if(sparam=="XauBridge_ForceMarketToggle")
        {
         g_ForceMarketOrders = !g_ForceMarketOrders;
         UpdateForceMarketButtonState();
         ChartRedraw();
        }
      // Lot Size Controls
      if(sparam=="XauBridge_LotPlus")
        {
         g_UserLotSize += 0.01;
         DrawDashboard();
        }
      if(sparam=="XauBridge_LotMinus")
        {
         g_UserLotSize -= 0.01;
         if(g_UserLotSize < 0.01)
            g_UserLotSize = 0.01;
         DrawDashboard();
        }
      // Max Entries Controls
      if(sparam=="XauBridge_EntPlus")
        {
         g_UserMaxEntries++;
         DrawDashboard();
        }
      if(sparam=="XauBridge_EntMinus")
        {
         g_UserMaxEntries--;
         if(g_UserMaxEntries < 1)
            g_UserMaxEntries = 1;
         DrawDashboard();
        }

      // Settings Panel Events
      if(sparam=="XauBridge_SetBack")
        {
         g_ShowSettings = false;
         ObjectsDeleteAll(0, "XauBridge_Set"); // Clean up settings UI
         ObjectDelete(0, "XauBridge_Stag. Secs");
         DrawDashboard();
        }
      if(sparam=="XauBridge_SetScalp")
        {
         g_UserScalpMode = !g_UserScalpMode;
         DrawDashboard();
        }
      if(sparam=="XauBridge_SetTrail")
        {
         g_UserTrailingSL = !g_UserTrailingSL;
         DrawDashboard();
        }
     if(sparam=="XauBridge_SetLiq")
       {
        g_ShowLiq = !g_ShowLiq;
        if(g_ShowLiq && g_ShowZones) DrawServerLiquidityZones(0, g_SignalState.jsonResponse);
        else ObjectsDeleteAll(0, "XauBridge_SrvLiq_");
        DrawDashboard();
        ChartRedraw();
       }
     if(sparam=="XauBridge_SetFVG")
       {
        g_ShowFVG = !g_ShowFVG;
        if(g_ShowFVG && g_ShowZones) DrawServerImbalances(0, g_SignalState.jsonResponse);
        else ObjectsDeleteAll(0, "XauBridge_SrvFVG_");
        DrawDashboard();
        ChartRedraw();
       }
     if(sparam=="XauBridge_SetOB")
       {
        g_ShowOB = !g_ShowOB;
        if(g_ShowOB && g_ShowZones) DrawServerOrderBlocks(0, g_SignalState.jsonResponse);
        else ObjectsDeleteAll(0, "XauBridge_SrvOB_");
        DrawDashboard();
        ChartRedraw();
       }
      if(sparam=="XauBridge_SetStagPlus")
        {
         g_UserStagnationSeconds += 10;
         DrawDashboard();
        }
      if(sparam=="XauBridge_SetStagMinus")
        {
         g_UserStagnationSeconds -= 10;
         if(g_UserStagnationSeconds < 10)
            g_UserStagnationSeconds = 10;
         DrawDashboard();
        }
      if(sparam=="XauBridge_SetReset")
        {
         g_UserScalpMode = ScalpMode;
         g_UserTrailingSL = UseTrailingSL;
         g_UserStagnationSeconds = StagnationSeconds;
         g_UserMaxEntries = MaxSignalEntries;
         g_UserLotSize = FixedLotSize;
        g_ShowLiq = true;
        g_ShowFVG = true;
        g_ShowOB = true;
         DrawDashboard();
        }
     }
  }
//+------------------------------------------------------------------+
