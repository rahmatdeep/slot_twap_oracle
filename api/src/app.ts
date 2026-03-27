import express from "express";
import rateLimit from "express-rate-limit";
import { config } from "./config";
import { requestLogger, errorHandler } from "./middleware";
import priceRouter from "./routes/price";
import twapRouter from "./routes/twap";
import historyRouter from "./routes/history";
import healthRouter from "./routes/health";

const app = express();

app.use(requestLogger);

const apiLimiter = rateLimit({
  windowMs: config.RATE_LIMIT_WINDOW_MS,
  max: config.RATE_LIMIT_MAX,
  standardHeaders: true,
  legacyHeaders: false,
  message: { error: "Too many requests, please try again later." },
});

app.use("/price", apiLimiter, priceRouter);
app.use("/twap", apiLimiter, twapRouter);
app.use("/history", apiLimiter, historyRouter);
app.use("/health", healthRouter); // no rate limit on health

app.use(errorHandler);

export default app;
