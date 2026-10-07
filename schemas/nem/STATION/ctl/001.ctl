-- ************************************************************ 
-- Title:	PUBLIC_DVD_STATION_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS STATION owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_STATION_201501010000.csv  
APPEND INTO TABLE STATION	
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
STATIONID CHAR(10), 
STATIONNAME CHAR(80), 
ADDRESS1 CHAR(80), 
ADDRESS2 CHAR(80), 
ADDRESS3 CHAR(80), 
ADDRESS4 CHAR(80), 
CITY CHAR(40), 
STATE CHAR(10), 
POSTCODE CHAR(10), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
CONNECTIONPOINTID CHAR(10)) 
-- End of Script 
