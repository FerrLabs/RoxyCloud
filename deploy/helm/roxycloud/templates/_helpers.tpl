{{- define "roxycloud.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "roxycloud.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else if contains (include "roxycloud.name" .) .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name (include "roxycloud.name" .) | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{- define "roxycloud.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{ include "roxycloud.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "roxycloud.selectorLabels" -}}
app.kubernetes.io/name: {{ include "roxycloud.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "roxycloud.databaseSecret" -}}
{{- default (include "roxycloud.fullname" .) .Values.database.existingSecret }}
{{- end }}

{{- define "roxycloud.jwtSecret" -}}
{{- default (include "roxycloud.fullname" .) .Values.jwt.existingSecret }}
{{- end }}

{{- define "roxycloud.claimName" -}}
{{- default (include "roxycloud.fullname" .) .Values.persistence.existingClaim }}
{{- end }}
