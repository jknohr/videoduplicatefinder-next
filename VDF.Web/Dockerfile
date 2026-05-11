# Build stage
FROM mcr.microsoft.com/dotnet/sdk:9.0 AS build
WORKDIR /src
COPY ["VDF.Core/VDF.Core.csproj", "VDF.Core/"]
COPY ["VDF.Web/VDF.Web.csproj", "VDF.Web/"]
RUN dotnet restore "VDF.Web/VDF.Web.csproj"
COPY VDF.Core/ VDF.Core/
COPY VDF.Web/ VDF.Web/
RUN dotnet publish "VDF.Web/VDF.Web.csproj" -c Release -o /app/publish --no-restore

# Runtime stage
FROM mcr.microsoft.com/dotnet/aspnet:9.0
# ffmpeg          — core media processing
# libva-drm2      — VA-API runtime (Intel / AMD GPU acceleration)
# libva-x11-2     — VA-API X11 support
# vainfo          — diagnostic: run "docker exec <id> vainfo" to verify VA-API works
# mesa-va-drivers — open-source VA-API drivers (AMD / Intel open driver)
# i965-va-driver  — Intel legacy VA-API driver (Gen 4–9)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg \
    libva-drm2 \
    libva-x11-2 \
    vainfo \
    mesa-va-drivers \
    i965-va-driver \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app/publish .
EXPOSE 8080
ENV ASPNETCORE_URLS=http://+:8080
ENTRYPOINT ["dotnet", "VDF.Web.dll"]
