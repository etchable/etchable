import { OpenAPIHono, createRoute, z } from "@hono/zod-openapi";
import { Scalar } from "@scalar/hono-api-reference";
import { drizzle } from "drizzle-orm/d1";
import { count } from "drizzle-orm";
import { waitlist } from "../db/schema";
import { createAuth } from "./auth";

const app = new OpenAPIHono<{ Bindings: Env }>();

// Drizzle wraps D1 errors, so the constraint message lives on the cause chain.
function isUniqueViolation(e: unknown): boolean {
  for (let err = e; err instanceof Error; err = err.cause) {
    if (err.message.includes("UNIQUE constraint failed")) return true;
  }
  return false;
}

app.on(["GET", "POST"], "/api/auth/*", (c) =>
  createAuth(c.env, new URL(c.req.url).origin).handler(c.req.raw),
);

const joinWaitlist = createRoute({
  method: "post",
  path: "/api/waitlist",
  request: {
    body: {
      content: {
        "application/json": {
          schema: z
            .object({ email: z.email().openapi({ example: "you@example.com" }) })
            .openapi("JoinWaitlist"),
        },
      },
    },
  },
  responses: {
    201: {
      description: "Joined the waitlist",
      content: {
        "application/json": {
          schema: z.object({ ok: z.literal(true) }).openapi("Joined"),
        },
      },
    },
    409: {
      description: "Already on the waitlist",
      content: {
        "application/json": {
          schema: z.object({ error: z.string() }).openapi("Conflict"),
        },
      },
    },
  },
});

const waitlistCount = createRoute({
  method: "get",
  path: "/api/waitlist/count",
  responses: {
    200: {
      description: "Number of people on the waitlist",
      content: {
        "application/json": {
          schema: z.object({ count: z.number() }).openapi("WaitlistCount"),
        },
      },
    },
  },
});

const routes = app
  .openapi(joinWaitlist, async (c) => {
    const { email } = c.req.valid("json");
    const db = drizzle(c.env.DB);
    try {
      await db.insert(waitlist).values({ email });
    } catch (e) {
      if (isUniqueViolation(e)) {
        return c.json({ error: "You're already on the list!" }, 409);
      }
      throw e;
    }
    return c.json({ ok: true as const }, 201);
  })
  .openapi(waitlistCount, async (c) => {
    const db = drizzle(c.env.DB);
    const [row] = await db.select({ count: count() }).from(waitlist);
    return c.json({ count: row?.count ?? 0 }, 200);
  });

app.doc("/api/openapi.json", {
  openapi: "3.1.0",
  info: {
    title: "etchable API",
    version: "0.1.0",
    description: "The API behind etchable.net",
  },
});

app.get("/api/docs", Scalar({ url: "/api/openapi.json" }));

export type AppType = typeof routes;

export default app;
