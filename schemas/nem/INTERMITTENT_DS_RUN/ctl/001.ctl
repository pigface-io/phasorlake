-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#INTERMITTENT_DS_RUN#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS INTERMITTENT_DS_RUN owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#INTERMITTENT_DS_RUN#FILE01#202408010000.CSV
APPEND INTO TABLE INTERMITTENT_DS_RUN
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
DUID CHAR(20),
OFFERDATETIME DATE "YYYY/MM/DD HH24:MI:SS",
ORIGIN CHAR(20),
FORECAST_PRIORITY FLOAT EXTERNAL,
AUTHORISEDBY CHAR(20),
COMMENTS CHAR(200),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
MODEL CHAR(30),
PARTICIPANT_TIMESTAMP DATE "YYYY/MM/DD HH24:MI:SS",
SUPPRESSED_AEMO FLOAT EXTERNAL,
SUPPRESSED_PARTICIPANT FLOAT EXTERNAL,
TRANSACTION_ID CHAR(100))
-- End of Script
