-- ************************************************************
-- Title:	PUBLIC_DVD_MTPASA_REGIONITERATION_201910.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MTPASA_REGIONITERATION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_MTPASA_REGIONITERATION_201910010000.CSV
APPEND INTO TABLE MTPASA_REGIONITERATION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
RUN_NO FLOAT EXTERNAL,
RUNTYPE CHAR(20),
DEMAND_POE_TYPE CHAR(20),
AGGREGATION_PERIOD CHAR(20),
PERIOD_ENDING DATE "yyyy/mm/dd hh24:mi:ss",
REGIONID CHAR(20),
USE_ITERATION_ID FLOAT EXTERNAL,
USE_ITERATION_EVENT_NUMBER FLOAT EXTERNAL,
USE_ITERATION_EVENT_AVERAGE FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
