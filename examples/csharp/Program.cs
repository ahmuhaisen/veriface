// Minimal Veriface client: POST /v1/verifications with HttpClient multipart, then map the
// result to an action. No third-party packages.
//
//   set VERIFACE_API_KEY=<key>                 (never hard-code or log the key)
//   set VERIFACE_URL=http://127.0.0.1:8080     (optional; this is the default)
//   dotnet run -- selfie.jpg id-card.jpg [profile]
//
// Contract: contracts/openapi.yaml (shapes) and contracts/checks.yaml (outcome rules).

using System.Net;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Veriface.Example;

public static class Program
{
    public static async Task<int> Main(string[] args)
    {
        if (args.Length is < 2 or > 3)
        {
            Console.Error.WriteLine("usage: veriface-example <selfie.jpg> <document.jpg> [profile]");
            return 2;
        }

        string? apiKey = Environment.GetEnvironmentVariable("VERIFACE_API_KEY");
        if (string.IsNullOrWhiteSpace(apiKey))
        {
            Console.Error.WriteLine("Set VERIFACE_API_KEY to the API key issued by the service operator.");
            return 2;
        }
        string baseUrl = Environment.GetEnvironmentVariable("VERIFACE_URL") ?? "http://127.0.0.1:8080";
        string profile = args.Length == 3 ? args[2] : "standard";

        // Slightly above the service's default request_timeout_ms (30 s), so the service decides.
        using var http = new HttpClient { BaseAddress = new Uri(baseUrl), Timeout = TimeSpan.FromSeconds(35) };
        var client = new FaceClient(http, apiKey);

        try
        {
            VerificationResult result = await client.VerifyAsync(
                File.ReadAllBytes(args[0]), File.ReadAllBytes(args[1]), profile);

            FaceAction action = OutcomeMapper.ToAction(result);
            Console.WriteLine($"requestId:   {result.RequestId}");
            Console.WriteLine($"outcome:     {result.Outcome} (decision {result.Decision})");
            Console.WriteLine($"calibration: {result.Calibration.Id} ({result.Calibration.Status})");
            Console.WriteLine($"profile:     {result.Profile.Name} {result.Profile.Version}");
            Console.WriteLine($"reasons:     {string.Join(", ", result.Reasons.Select(r => $"{r.Source}:{r.Code}"))}");
            Console.WriteLine($"action:      {action}");
            // The score is deliberately not printed: never show it to end users, and keep it out
            // of general-purpose logs.
            return 0;
        }
        catch (FaceServiceException e)
        {
            Console.Error.WriteLine($"Face service error {e.Status} {e.Code} (request {e.RequestId}): {e.Message}");
            return 1;
        }
        catch (HttpRequestException e)
        {
            Console.Error.WriteLine($"Face service unreachable: {e.Message}");
            return 1;
        }
        catch (TaskCanceledException)
        {
            Console.Error.WriteLine("Face service did not answer in time.");
            return 1;
        }
    }
}

/// <summary>What the integrating project should do next. See docs/integration-guide.md section 6.</summary>
public enum FaceAction
{
    /// <summary>outcome = verified: continue the flow.</summary>
    Accept,
    /// <summary>outcome = retake, a probe check failed: ask for a new selfie.</summary>
    RetakeSelfie,
    /// <summary>outcome = retake, a reference check failed: ask for a new document photo.</summary>
    RetakeDocument,
    /// <summary>outcome = review: route to a trained reviewer.</summary>
    HumanReview,
    /// <summary>outcome = rejected: do not accept automatically; allow a limited retry, then review.</summary>
    Reject,
}

public static class OutcomeMapper
{
    public static FaceAction ToAction(VerificationResult result) => result.Outcome switch
    {
        "verified" => FaceAction.Accept,
        // Reasons are most important first; the first one says which capture to repeat.
        "retake" => result.Reasons.FirstOrDefault()?.Source == "reference"
            ? FaceAction.RetakeDocument
            : FaceAction.RetakeSelfie,
        "review" => FaceAction.HumanReview,
        "rejected" => FaceAction.Reject,
        // An outcome this client does not know (a newer service): fail safe.
        _ => FaceAction.HumanReview,
    };
}

/// <summary>A thin client for POST /v1/verifications.</summary>
public sealed class FaceClient(HttpClient http, string apiKey)
{
    private const int MaxAttempts = 3;

    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);

    public async Task<VerificationResult> VerifyAsync(
        byte[] probeJpeg, byte[] referenceJpeg, string profile, CancellationToken cancellationToken = default)
    {
        string path = $"/v1/verifications?profile={Uri.EscapeDataString(profile)}&referenceKind=auto";
        // One id for every attempt, so the service log ties the retries together.
        string requestId = Guid.NewGuid().ToString();

        for (int attempt = 1; ; attempt++)
        {
            // A multipart body can be sent only once, so build a new one per attempt.
            using var form = new MultipartFormDataContent();
            form.Add(Image(probeJpeg), "probe", "probe.jpg");
            form.Add(Image(referenceJpeg), "reference", "reference.jpg");

            using var request = new HttpRequestMessage(HttpMethod.Post, path) { Content = form };
            request.Headers.Add("X-API-Key", apiKey);
            request.Headers.Add("X-Request-Id", requestId);
            // Lets the service refuse a request (401, 403, 404, 503 ...) before the images are uploaded;
            // without it, an early refusal can surface as a broken upload instead of a problem response.
            request.Headers.ExpectContinue = true;

            using HttpResponseMessage response = await http.SendAsync(request, cancellationToken);
            if (response.IsSuccessStatusCode)
            {
                VerificationResult? result = await response.Content.ReadFromJsonAsync<VerificationResult>(
                    Json, cancellationToken);
                return result ?? throw new FaceServiceException(200, "invalid-response", requestId, "empty body");
            }

            Problem? problem = await ReadProblemAsync(response, cancellationToken);
            string code = problem?.Code ?? "unknown";

            // 503 overloaded / not-ready are retryable; honour Retry-After plus jitter.
            if (response.StatusCode == HttpStatusCode.ServiceUnavailable && attempt < MaxAttempts)
            {
                TimeSpan wait = response.Headers.RetryAfter?.Delta ?? TimeSpan.FromSeconds(1);
                wait += TimeSpan.FromMilliseconds(Random.Shared.Next(0, 500));
                await Task.Delay(wait, cancellationToken);
                continue;
            }

            // Everything else (4xx, 500, or 503 after the last attempt) goes back to the caller.
            throw new FaceServiceException(
                (int)response.StatusCode, code, problem?.Instance ?? requestId, problem?.Title ?? response.ReasonPhrase ?? "");
        }
    }

    private static ByteArrayContent Image(byte[] jpeg)
    {
        var content = new ByteArrayContent(jpeg);
        content.Headers.ContentType = new MediaTypeHeaderValue("image/jpeg");
        return content;
    }

    private static async Task<Problem?> ReadProblemAsync(HttpResponseMessage response, CancellationToken ct)
    {
        try
        {
            return await response.Content.ReadFromJsonAsync<Problem>(Json, ct);
        }
        catch (JsonException)
        {
            return null;
        }
    }
}

public sealed class FaceServiceException(int status, string code, string requestId, string message)
    : Exception(message)
{
    public int Status { get; } = status;
    /// <summary>The RFC 9457 problem code, e.g. overloaded, profile-forbidden, image-decode-failed.</summary>
    public string Code { get; } = code;
    public string RequestId { get; } = requestId;
}

// ---------------------------------------------------------------- response shapes (subset)
// Only the fields this example uses. Unknown fields are ignored, so new optional fields in /v1
// responses do not break the client.

public sealed record VerificationResult(
    string RequestId,
    string Outcome,
    string Decision,
    double? Score,
    double Threshold,
    OperatingPointRef OperatingPoint,
    CalibrationRef Calibration,
    IReadOnlyList<VerificationReason> Reasons,
    RiskSummary Risk,
    ProfileRef Profile);

public sealed record OperatingPointRef(string Id, double TargetFmr);

public sealed record CalibrationRef(string Id, string Status);

/// <summary>Source is probe, reference, risk or decision; code is a check id, SCORE_BELOW_THRESHOLD or NO_SCORE.</summary>
public sealed record VerificationReason(string Source, string Code);

public sealed record RiskSummary(bool Flagged);

public sealed record ProfileRef(string Name, string Version, string Sha256);

public sealed record Problem(
    string Type,
    string Title,
    int Status,
    string Code,
    string? Detail,
    [property: JsonPropertyName("instance")] string? Instance);
